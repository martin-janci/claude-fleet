// Mouse forwarding and local drag-selection for TerminalView (moved out of
// TerminalView.svelte, F5b). The selection and every other reactive value
// stay in the component; this controller reads and writes them through
// `MouseHost` and owns only the in-progress gesture state.
import { encodeMouse, type Screen } from './ansi';
import type { CellPos } from './terminal_selection';
import { copyOnSelect } from './prefs';
import { get } from 'svelte/store';

export interface MouseHost {
  ptyOpen(): boolean;
  screen(): Screen | null;
  container(): HTMLElement | undefined;
  lastCols(): number;
  lastRows(): number;
  cellWidth(): number;
  cellHeight(): number;
  selAnchor(): CellPos | null;
  selFocus(): CellPos | null;
  setSelAnchor(cell: CellPos | null): void;
  setSelFocus(cell: CellPos | null): void;
  clearSelection(): void;
  copySelection(): Promise<void>;
  writePty(data: string): void;
}

const WHEEL_TICK_PX = 40;
/** Pointer travel (px) before a deferred left-press promotes to a selection. */
const DRAG_PX = 4;

export function createMouseController(host: MouseHost) {
  /** True while a drag-select is in progress (between mousedown and mouseup). */
  let selecting = false;

  // ─── Mouse forwarding state ───────────────────────────────────────────
  /** Which button (0/1/2, encoded as cb) is currently pressed. Null = none. */
  let pressedButton: number | null = null;
  /** When mouse reporting is on, a left press is deferred until we know whether
   *  it becomes a drag (→ local selection) or a click (→ forward to the app). */
  let pendingPress: { cell: CellPos; startX: number; startY: number } | null = null;
  /** The last cell (1-based col, row) for which we sent a motion report,
   *  used to throttle: we only send a new report when the cell changes. */
  let lastMotionCell: { col: number; row: number } | null = null;
  /** Cleanup functions for the window-level mousemove/mouseup listeners added
   *  on mousedown. Removed on mouseup or component destroy. */
  let removeWindowListeners: (() => void) | null = null;
  /** Accumulated (pixel-normalized) wheel delta not yet turned into reports.
   *  We forward one wheel report per WHEEL_TICK_PX of scroll instead of one
   *  per event, so trackpads (many tiny deltas) don't flood tmux and line-mode
   *  wheels still register — smooth, proportional scrolling either way. */
  let wheelAccum = 0;

  /** Map a MouseEvent's client coordinates to a 1-based terminal cell,
   *  clamped to the visible grid. Accounts for the 4px left/top padding. */
  function eventToCell(e: MouseEvent): { col: number; row: number } {
    const rect = host.container()!.getBoundingClientRect();
    const col = Math.max(1, Math.min(host.lastCols(),
      Math.floor((e.clientX - rect.left - 4) / host.cellWidth()) + 1));
    const row = Math.max(1, Math.min(host.lastRows(),
      Math.floor((e.clientY - rect.top - 4) / host.cellHeight()) + 1));
    return { col, row };
  }

  /** Write a mouse escape sequence to the PTY. */
  function sendMouse(data: string) {
    host.writePty(data);
  }

  /** Convert a 1-based eventToCell result to a 0-based grid cell. */
  function cellFromEvent(e: MouseEvent): CellPos {
    const { col, row } = eventToCell(e);
    return { row: row - 1, col: col - 1 };
  }

  function onWheel(e: WheelEvent) {
    if (e.altKey) return;
    const screen = host.screen();
    if (!host.ptyOpen() || !screen || !screen.mouseEnabled) return;
    e.preventDefault();
    // Normalize the delta to pixels across deltaMode (0=px, 1=lines, 2=pages)
    // so wheels and trackpads accumulate on the same scale.
    const line = host.cellHeight() || 16;
    let dy = e.deltaY;
    if (e.deltaMode === 1) dy *= line;
    else if (e.deltaMode === 2) dy *= line * (host.lastRows() || 24);
    // Reset on direction change so a flip registers immediately.
    if ((dy < 0 && wheelAccum > 0) || (dy > 0 && wheelAccum < 0)) wheelAccum = 0;
    wheelAccum += dy;
    const { col, row } = eventToCell(e);
    const sgr = screen.mouseSgr;
    // Emit one wheel report per WHEEL_TICK_PX of accumulated scroll. Batch all
    // reports for this event into a single PTY write; guard caps a pathological
    // delta at 64 reports.
    let reports = '';
    let guard = 0;
    while (Math.abs(wheelAccum) >= WHEEL_TICK_PX && guard++ < 64) {
      const up = wheelAccum < 0;
      reports += encodeMouse(up ? 64 : 65, col, row, false, sgr);
      wheelAccum += up ? WHEEL_TICK_PX : -WHEEL_TICK_PX;
    }
    if (reports) sendMouse(reports);
  }

  function onMousedown(e: MouseEvent) {
    const screen = host.screen();
    if (!host.ptyOpen() || !screen) return;
    // Right-click is reserved for our context menu (handled by onContextMenu).
    if (e.button === 2) return;
    // Left-button only for selection; other buttons fall through to app forwarding.
    if (e.button === 0 && !screen.mouseEnabled && !e.altKey) {
      // Plain shell: begin a local drag-selection.
      e.preventDefault();
      // Tear down any prior in-progress drag before starting a new one, so a
      // missed mouseup can't leave a stale handler that wipes this selection.
      removeWindowListeners?.();
      (e.currentTarget as HTMLElement | null)?.focus();
      const cell = cellFromEvent(e);
      host.setSelAnchor(cell);
      host.setSelFocus(cell);
      selecting = true;
      const handleMove = (ev: MouseEvent) => {
        if (!selecting) return;
        host.setSelFocus(cellFromEvent(ev));
      };
      const handleUp = () => {
        if (!selecting) return;
        selecting = false;
        removeWindowListeners?.();
        // Only copy a real drag-selection; a plain click clears any selection.
        const selAnchor = host.selAnchor();
        const selFocus = host.selFocus();
        if (
          selAnchor && selFocus &&
          (selAnchor.row !== selFocus.row || selAnchor.col !== selFocus.col)
        ) {
          if (get(copyOnSelect)) void host.copySelection();
        } else {
          host.clearSelection();
        }
      };
      window.addEventListener('mousemove', handleMove);
      window.addEventListener('mouseup', handleUp);
      removeWindowListeners = () => {
        window.removeEventListener('mousemove', handleMove);
        window.removeEventListener('mouseup', handleUp);
        removeWindowListeners = null;
      };
      return;
    }
    // Mouse reporting ON, left button, no Option → defer: a drag becomes a local
    // selection, a click (no movement) forwards to the app.
    if (e.button === 0 && screen.mouseEnabled && !e.altKey) {
      e.preventDefault();
      removeWindowListeners?.(); // drop any stale in-progress drag first
      (e.currentTarget as HTMLElement | null)?.focus();
      const cell = cellFromEvent(e);
      pendingPress = { cell, startX: e.clientX, startY: e.clientY };
      host.clearSelection();
      const handleMove = (ev: MouseEvent) => {
        if (!pendingPress) return;
        const moved =
          Math.abs(ev.clientX - pendingPress.startX) > DRAG_PX ||
          Math.abs(ev.clientY - pendingPress.startY) > DRAG_PX;
        if (moved && !selecting) {
          // Promote to a local selection.
          selecting = true;
          host.setSelAnchor(pendingPress.cell);
        }
        if (selecting) host.setSelFocus(cellFromEvent(ev));
      };
      const handleUp = (ev: MouseEvent) => {
        removeWindowListeners?.();
        if (selecting) {
          selecting = false;
          if (get(copyOnSelect)) void host.copySelection();
        } else if (pendingPress) {
          // No drag → forward a real click (press + release) to the app.
          const c = cellFromEvent(ev);
          const sgr = host.screen()!.mouseSgr;
          // Press + release in one write so the app sees an atomic click.
          sendMouse(
            encodeMouse(0, c.col + 1, c.row + 1, false, sgr) +
              encodeMouse(0, c.col + 1, c.row + 1, true, sgr),
          );
        }
        pendingPress = null;
      };
      window.addEventListener('mousemove', handleMove);
      window.addEventListener('mouseup', handleUp);
      removeWindowListeners = () => {
        window.removeEventListener('mousemove', handleMove);
        window.removeEventListener('mouseup', handleUp);
        removeWindowListeners = null;
      };
      return;
    }
    // Not a left-button local-select gesture. Option-held drags and middle/right
    // buttons fall through to app forwarding below.
    if (!screen.mouseEnabled) return; // no reporting + not a left-select → ignore
    // Only forward left (0), middle (1), right (2).
    if (e.button > 2) return;
    e.preventDefault();
    // preventDefault() suppresses the browser's default focus-on-click; focus
    // the terminal explicitly so keystrokes keep flowing after a mouse-mode click.
    (e.currentTarget as HTMLElement | null)?.focus();
    const { col, row } = eventToCell(e);
    const cb = e.button; // 0=left 1=middle 2=right
    pressedButton = cb;
    lastMotionCell = { col, row };
    const sgr = screen.mouseSgr;
    sendMouse(encodeMouse(cb, col, row, false, sgr));

    // Attach window-level listeners so we keep tracking if the pointer
    // leaves the terminal element before the button is released.
    const handleMove = (ev: MouseEvent) => onWindowMousemove(ev);
    const handleUp   = (ev: MouseEvent) => onWindowMouseup(ev);
    window.addEventListener('mousemove', handleMove);
    window.addEventListener('mouseup', handleUp);
    removeWindowListeners = () => {
      window.removeEventListener('mousemove', handleMove);
      window.removeEventListener('mouseup', handleUp);
      removeWindowListeners = null;
    };
  }

  function onWindowMousemove(e: MouseEvent) {
    const screen = host.screen();
    if (!host.ptyOpen() || !screen || pressedButton === null && !screen.mouseAnyMotion) return;
    const { col, row } = eventToCell(e);
    // Throttle: only send a report if the cell actually changed.
    if (lastMotionCell && lastMotionCell.col === col && lastMotionCell.row === row) return;
    lastMotionCell = { col, row };
    const sgr = screen.mouseSgr;
    if (pressedButton !== null && screen.mouseButtonMotion) {
      // Button held — report as motion with the pressed button.
      sendMouse(encodeMouse(pressedButton + 32, col, row, false, sgr));
    } else if (pressedButton === null && screen.mouseAnyMotion) {
      // No button held — any-motion mode (cb = 3 + 32 = 35).
      sendMouse(encodeMouse(35, col, row, false, sgr));
    }
  }

  function onWindowMouseup(e: MouseEvent) {
    if (pressedButton === null) return;
    const screen = host.screen();
    if (host.ptyOpen() && screen && screen.mouseEnabled && host.container()) {
      const { col, row } = eventToCell(e);
      const sgr = screen.mouseSgr;
      sendMouse(encodeMouse(pressedButton, col, row, true, sgr));
    }
    pressedButton = null;
    lastMotionCell = null;
    removeWindowListeners?.();
  }

  /** Reset any in-progress drag state (on a session switch). */
  function reset() {
    selecting = false;
    pendingPress = null;
  }

  /** Remove the window-level listeners of a gesture still in progress. */
  function dispose() {
    removeWindowListeners?.();
  }

  return { onWheel, onMousedown, reset, dispose };
}
