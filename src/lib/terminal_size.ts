// The terminal grid's minimum size, and the fit that enforces it.
//
// MIN_COLS/MIN_ROWS MUST stay equal to `MIN_COLS`/`MIN_ROWS` in
// src-tauri/src/pty.rs, which clamps every `pty_open` and `pty_resize`. The
// two are one contract, not two guards: when the backend clamped higher than
// the renderer (it used to floor at 40×10 while this floored at 10×2) tmux
// drew for a grid the Screen did not have, so in a narrow pane output wrapped,
// scrolled and positioned the cursor for the wrong geometry and the top rows —
// tmux's status line included — never appeared.
//
// The floor exists only to keep a degenerate size off the PTY: a pane that has
// not been laid out yet reports 0×0.

export const MIN_COLS = 10;
export const MIN_ROWS = 2;

/** Grid size for a content box of `widthPx` × `heightPx`, given the measured
 *  cell metrics. Never returns less than the shared minimum. */
export function fitCells(
  widthPx: number,
  heightPx: number,
  cellWidth: number,
  cellHeight: number,
): { cols: number; rows: number } {
  return {
    cols: Math.max(MIN_COLS, Math.floor(widthPx / cellWidth)),
    rows: Math.max(MIN_ROWS, Math.floor(heightPx / cellHeight)),
  };
}
