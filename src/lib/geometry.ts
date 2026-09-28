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

/** Whether this webview runs on Windows (WebView2 reports `Win32` and a
 *  `Windows NT` user agent). */
export function detectWindows(nav: { platform?: string; userAgent?: string } | undefined): boolean {
  if (!nav) return false;
  return /^Win/.test(nav.platform ?? '') || /Windows NT/.test(nav.userAgent ?? '');
}

/** A Tauri drag-drop `position` in the logical pixels `getBoundingClientRect()`
 *  uses. macOS (AppKit points) and Linux (GTK widget coordinates) already
 *  deliver logical pixels — see `pointInRect`. Windows does not: wry's
 *  WebView2 drop target runs `ScreenToClient`, which gives PHYSICAL client
 *  pixels, so at 150% scaling a drop on the lower-right of the terminal read
 *  as outside it. There, and only there, divide by the scale factor. */
export function dropPointToLogical(
  pos: { x: number; y: number },
  windows: boolean,
  devicePixelRatio: number,
): { x: number; y: number } {
  if (!windows || !(devicePixelRatio > 0)) return { x: pos.x, y: pos.y };
  return { x: pos.x / devicePixelRatio, y: pos.y / devicePixelRatio };
}

/** `dropPointToLogical` for this webview. */
export function dropPoint(pos: { x: number; y: number }): { x: number; y: number } {
  const nav = typeof navigator === 'undefined' ? undefined : navigator;
  const dpr = typeof window === 'undefined' ? 1 : window.devicePixelRatio;
  return dropPointToLogical(pos, detectWindows(nav), dpr);
}
