// Tiny geometry helpers, unit-tested directly. Kept separate from the
// Svelte component so the coordinate contract can be asserted without a DOM.

/** A logical-pixel rectangle, as returned by `Element.getBoundingClientRect()`. */
export interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** Is the logical point (px, py) inside `rect` (inclusive of edges)?
 *
 *  IMPORTANT: both the point and the rect must be in the same coordinate
 *  space — CSS/logical pixels with a top-left origin. Tauri's drag-drop
 *  `position` on macOS is delivered in logical points (wry hands AppKit
 *  points to tauri-runtime-wry, which wraps them in a `PhysicalPosition`
 *  WITHOUT applying the scale factor), so it must NOT be divided by
 *  `devicePixelRatio` before being compared against `getBoundingClientRect()`.
 *  Doing so halves the coordinates on a 2× Retina display and the hit-test
 *  misses everywhere. */
export function pointInRect(px: number, py: number, rect: Rect): boolean {
  return px >= rect.left && px <= rect.right && py >= rect.top && py <= rect.bottom;
}
