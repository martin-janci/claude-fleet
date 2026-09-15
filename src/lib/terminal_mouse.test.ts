import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { createMouseController } from './terminal_mouse';
import { Screen } from './ansi';
import type { CellPos } from './terminal_selection';

/** A controller wired to a real Screen and a real (zero-rect) container, with
 *  every PTY write recorded. `modes` seeds the app's mouse reporting: 1000
 *  (clicks), 1002 (button motion), 1003 (any motion), 1006 (SGR) — what a
 *  Claude pane asks tmux for. */
function setup(modes = '\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h') {
  const screen = new Screen(10, 20);
  screen.write(modes);
  const container = document.createElement('div');
  document.body.appendChild(container);
  const writes: string[] = [];
  let selAnchor: CellPos | null = null;
  let selFocus: CellPos | null = null;
  const mouse = createMouseController({
    ptyOpen: () => true,
    screen: () => screen,
    container: () => container,
    lastCols: () => 20,
    lastRows: () => 10,
    cellWidth: () => 8,
    cellHeight: () => 16,
    selAnchor: () => selAnchor,
    selFocus: () => selFocus,
    setSelAnchor: (c) => (selAnchor = c),
    setSelFocus: (c) => (selFocus = c),
    clearSelection: () => {
      selAnchor = null;
      selFocus = null;
    },
    copySelection: async () => {},
    writePty: (d) => writes.push(d),
  });
  return { mouse, writes, container, screen, sel: () => ({ anchor: selAnchor, focus: selFocus }) };
}

const down = (button: number, init: MouseEventInit = {}) =>
  new MouseEvent('mousedown', { bubbles: true, cancelable: true, button, detail: 1, ...init });

function windowMove(x: number, y = 20) {
  window.dispatchEvent(new MouseEvent('mousemove', { clientX: x, clientY: y, bubbles: true }));
}
function windowUp(x = 20, y = 20) {
  window.dispatchEvent(new MouseEvent('mouseup', { clientX: x, clientY: y, bubbles: true }));
}

describe('createMouseController window listeners (N6)', () => {
  let controllers: Array<{ dispose: () => void }> = [];
  beforeEach(() => {
    controllers = [];
  });
  afterEach(() => {
    for (const c of controllers) c.dispose();
    document.body.innerHTML = '';
  });

  it('a second forwarded press does not orphan the first press listeners', () => {
    const { mouse, writes } = setup();
    controllers.push(mouse);
    // A chord: middle button held, then Option+left. Both forward.
    mouse.onMousedown(down(1));
    mouse.onMousedown(down(0, { altKey: true }));
    windowUp(20, 20);
    writes.length = 0;
    // Pointer moves on with no button held. With any-motion reporting on, an
    // orphaned listener would forward a CSI < 35 report for each of these.
    for (const x of [30, 50, 70, 90]) windowMove(x);
    expect(writes).toEqual([]);
  });

  it('reset() during a held gesture drops its listeners', () => {
    const { mouse, writes } = setup();
    controllers.push(mouse);
    mouse.onMousedown(down(1));
    // A session switch resets the controller mid-drag; the mouseup that would
    // have torn the listeners down arrives afterwards, or never.
    mouse.reset();
    writes.length = 0;
    for (const x of [30, 50, 70]) windowMove(x);
    expect(writes).toEqual([]);
    windowUp(70, 20);
    for (const x of [30, 50, 70]) windowMove(x);
    expect(writes).toEqual([]);
  });

  it('a local drag-select interrupted by reset() leaves nothing on window', () => {
    // The plain-shell path: no mouse reporting, so the press starts a local
    // selection whose handleUp early-returns once reset() cleared `selecting`.
    const { mouse, writes, screen } = setup('');
    controllers.push(mouse);
    mouse.onMousedown(down(0));
    mouse.reset();
    windowUp();
    // Now the app turns reporting on and a forwarded press comes in.
    screen.write('\x1b[?1000h\x1b[?1003h\x1b[?1006h');
    mouse.onMousedown(down(1));
    windowUp();
    writes.length = 0;
    for (const x of [30, 50, 70]) windowMove(x);
    expect(writes).toEqual([]);
  });

  it('still forwards motion while a button is genuinely held', () => {
    const { mouse, writes } = setup();
    controllers.push(mouse);
    mouse.onMousedown(down(1)); // press report
    writes.length = 0;
    windowMove(30);
    windowMove(60);
    expect(writes).toHaveLength(2);
    expect(writes.every((w) => w.startsWith('\x1b[<33;'))).toBe(true); // 1 + 32
    windowUp(60);
    expect(writes).toHaveLength(3);
    expect(writes[2]).toMatch(/^\x1b\[<1;.*m$/); // SGR release
  });

  it('a chord over a drag-select does not swallow the next click', () => {
    const { mouse, writes } = setup();
    controllers.push(mouse);
    // Reporting is on, so the left press is deferred; the move promotes it to
    // a local drag-select that the middle button then pre-empts — that
    // gesture's mouseup never runs.
    mouse.onMousedown(down(0, { clientX: 10, clientY: 20 }));
    windowMove(100);
    mouse.onMousedown(down(1, { clientX: 100, clientY: 20 }));
    windowUp(100, 20);
    writes.length = 0;
    // The next plain click must reach the app, not be eaten as the tail of the
    // drag that was pre-empted.
    mouse.onMousedown(down(0, { clientX: 10, clientY: 20 }));
    windowUp(10, 20);
    expect(writes.join('')).toMatch(/\x1b\[<0;1;2M/);
  });

  it('a drag after a chord anchors where it started, not where the dead one did', () => {
    const { mouse, sel } = setup();
    controllers.push(mouse);
    mouse.onMousedown(down(0, { clientX: 50, clientY: 20 })); // cell col 5
    windowMove(100);
    mouse.onMousedown(down(1, { clientX: 100, clientY: 20 })); // pre-empts it
    windowUp(100, 20);
    // A fresh drag from the left edge. The pre-empted gesture's anchor must be
    // gone, or this one extends from cell 5 instead of cell 0.
    mouse.onMousedown(down(0, { clientX: 10, clientY: 20 })); // cell col 0
    windowMove(30); // cell col 3
    expect(sel().anchor).toEqual({ row: 1, col: 0 });
    expect(sel().focus).toEqual({ row: 1, col: 3 });
    windowUp(30, 20);
  });

  it('dispose() removes the listeners of a gesture still in progress', () => {
    const { mouse, writes } = setup();
    mouse.onMousedown(down(1));
    mouse.dispose();
    writes.length = 0;
    for (const x of [30, 50]) windowMove(x);
    windowUp(50);
    expect(writes).toEqual([]);
  });
});
