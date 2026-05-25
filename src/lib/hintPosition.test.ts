import { describe, it, expect } from 'vitest';
import { computeBubblePosition, type Rect } from './hintPosition';

const anchor: Rect = { top: 100, left: 100, width: 40, height: 20 };

describe('computeBubblePosition', () => {
  it('places a bottom bubble below and horizontally centered on the anchor', () => {
    const p = computeBubblePosition(anchor, 'bottom', 200, 80, 1000, 1000);
    expect(p.top).toBe(100 + 20 + 8); // below with gap
    expect(p.left).toBe(100 + 20 - 100); // center: anchorCenterX - bubbleW/2 = 120 - 100
  });

  it('places a top bubble above the anchor', () => {
    const p = computeBubblePosition(anchor, 'top', 200, 80, 1000, 1000);
    expect(p.top).toBe(100 - 80 - 8);
  });

  it('clamps a bubble that would overflow the right edge', () => {
    const right: Rect = { top: 100, left: 980, width: 40, height: 20 };
    const p = computeBubblePosition(right, 'bottom', 200, 80, 1000, 1000);
    expect(p.left).toBe(1000 - 200 - 6); // vw - bubbleW - MARGIN
  });

  it('clamps a bubble that would overflow the top edge', () => {
    const top: Rect = { top: 2, left: 100, width: 40, height: 20 };
    const p = computeBubblePosition(top, 'top', 200, 80, 1000, 1000);
    expect(p.top).toBe(6); // MARGIN
  });
});
