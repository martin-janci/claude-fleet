import { describe, it, expect } from 'vitest';
import { pointInRect, type Rect } from './geometry';

describe('pointInRect', () => {
  // A terminal grid sitting to the right of the sidebar, in logical px.
  const grid: Rect = { left: 260, top: 40, right: 1280, bottom: 800 };

  it('accepts a point inside the rect', () => {
    expect(pointInRect(770, 420, grid)).toBe(true);
  });

  it('accepts points exactly on the edges', () => {
    expect(pointInRect(260, 40, grid)).toBe(true);
    expect(pointInRect(1280, 800, grid)).toBe(true);
  });

  it('rejects a point left of / above the rect', () => {
    expect(pointInRect(100, 20, grid)).toBe(false);
  });

  it('rejects a point right of / below the rect', () => {
    expect(pointInRect(1300, 820, grid)).toBe(false);
  });

  // Regression guard for the Retina drag-drop bug: the macOS drag position is
  // already logical, so a drop must register at its true coordinates. The old
  // code divided by devicePixelRatio (2 on Retina), turning a drop near the
  // grid's top-left — e.g. (480, 70) — into (240, 35), which falls left of and
  // above the grid and misses.
  it('does not require any devicePixelRatio scaling of the point', () => {
    const trueDrop = { x: 480, y: 70 };
    const halved = { x: trueDrop.x / 2, y: trueDrop.y / 2 };
    expect(pointInRect(trueDrop.x, trueDrop.y, grid)).toBe(true);
    // The halved point lands outside the grid (left of and above) — the bug.
    expect(pointInRect(halved.x, halved.y, grid)).toBe(false);
  });
});
