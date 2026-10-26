import { describe, it, expect } from 'vitest';
import { tabStrip } from './tab_strip';

// jsdom lays nothing out, so each test sets the strip's geometry by hand.
function strip(scrollWidth: number, clientWidth: number) {
  const node = document.createElement('div');
  node.innerHTML = '<button role="tab" aria-selected="false">A</button><button role="tab" aria-selected="true">B</button>';
  Object.defineProperty(node, 'scrollWidth', { value: scrollWidth });
  Object.defineProperty(node, 'clientWidth', { value: clientWidth });
  const sel = node.querySelectorAll<HTMLElement>('[role="tab"]')[1];
  Object.defineProperty(sel, 'offsetLeft', { value: 300 });
  Object.defineProperty(sel, 'offsetWidth', { value: 60 });
  document.body.append(node);
  return node;
}

describe('tabStrip', () => {
  it('scrolls the selected tab into view and marks the edges with more past them', async () => {
    const node = strip(600, 200);
    const a = tabStrip(node);
    await Promise.resolve();
    expect(node.scrollLeft).toBe(300 + 60 - 200 + 16);
    expect(node.hasAttribute('data-more-left')).toBe(true);
    expect(node.hasAttribute('data-more-right')).toBe(true);
    a.destroy();
  });

  it('a vertical wheel scrolls it sideways, and a strip that fits marks nothing', async () => {
    const wide = strip(600, 200);
    tabStrip(wide);
    await Promise.resolve();
    const before = wide.scrollLeft;
    wide.dispatchEvent(new WheelEvent('wheel', { deltaY: 40, cancelable: true }));
    expect(wide.scrollLeft).toBe(before + 40);

    const fits = strip(200, 200);
    tabStrip(fits);
    await Promise.resolve();
    expect(fits.hasAttribute('data-more-left')).toBe(false);
    expect(fits.hasAttribute('data-more-right')).toBe(false);
  });
});
