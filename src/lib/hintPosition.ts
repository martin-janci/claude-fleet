import type { Placement } from './hints';

export interface Rect {
  top: number;
  left: number;
  width: number;
  height: number;
}

export interface Pos {
  top: number;
  left: number;
}

const GAP = 8; // px between anchor and bubble
const MARGIN = 6; // px min distance from viewport edge

/**
 * Position a bubble of size `bubbleW`×`bubbleH` relative to `anchor` for the
 * given `placement`, clamped to stay within the `vw`×`vh` viewport. Pure — all
 * inputs are plain numbers so it is unit-testable without a DOM.
 */
export function computeBubblePosition(
  anchor: Rect,
  placement: Placement,
  bubbleW: number,
  bubbleH: number,
  vw: number,
  vh: number,
): Pos {
  let top: number;
  let left: number;

  switch (placement) {
    case 'top':
      top = anchor.top - bubbleH - GAP;
      left = anchor.left + anchor.width / 2 - bubbleW / 2;
      break;
    case 'left':
      top = anchor.top + anchor.height / 2 - bubbleH / 2;
      left = anchor.left - bubbleW - GAP;
      break;
    case 'right':
      top = anchor.top + anchor.height / 2 - bubbleH / 2;
      left = anchor.left + anchor.width + GAP;
      break;
    case 'bottom':
    default:
      top = anchor.top + anchor.height + GAP;
      left = anchor.left + anchor.width / 2 - bubbleW / 2;
      break;
  }

  // Clamp to viewport.
  left = Math.max(MARGIN, Math.min(left, vw - bubbleW - MARGIN));
  top = Math.max(MARGIN, Math.min(top, vh - bubbleH - MARGIN));
  return { top, left };
}
