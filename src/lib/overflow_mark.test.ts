import { describe, it, expect } from 'vitest';
import { markOverflow } from './overflow_mark';

// The tray above Control's composer fades its bottom edge while it holds
// more than it shows (Control chat UX, 2026-10-10).
function box(scrollHeight: number, clientHeight: number, scrollTop = 0): HTMLElement {
  const el = document.createElement('div');
  Object.defineProperty(el, 'scrollHeight', { value: scrollHeight });
  Object.defineProperty(el, 'clientHeight', { value: clientHeight });
  el.scrollTop = scrollTop;
  Object.defineProperty(el, 'scrollTop', { value: scrollTop });
  return el;
}

describe('markOverflow', () => {
  it('marks a box that fits as not overflowing', () => {
    const el = box(100, 100);
    markOverflow(el);
    expect(el.dataset.overflow).toBe('false');
    expect(el.dataset.atEnd).toBe('true');
  });

  it('marks an overflowing box until it is scrolled to the end', () => {
    const top = box(400, 200, 0);
    markOverflow(top);
    expect(top.dataset.overflow).toBe('true');
    expect(top.dataset.atEnd).toBe('false');
    const end = box(400, 200, 200);
    markOverflow(end);
    expect(end.dataset.atEnd).toBe('true');
  });
});
