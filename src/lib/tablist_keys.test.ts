import { readFileSync, readdirSync } from 'node:fs';
import { describe, it, expect, vi } from 'vitest';
import { tablistKeys } from './tablist_keys';

function strip(selected = 0, disabled: number[] = []) {
  const list = document.createElement('div');
  list.setAttribute('role', 'tablist');
  const clicks: number[] = [];
  const tabs = [0, 1, 2].map((i) => {
    const b = document.createElement('button');
    b.setAttribute('role', 'tab');
    b.setAttribute('aria-selected', String(i === selected));
    if (disabled.includes(i)) b.setAttribute('disabled', '');
    b.addEventListener('click', () => clicks.push(i));
    list.append(b);
    return b;
  });
  const extra = document.createElement('button'); // a close or + button in the strip
  list.append(extra);
  document.body.append(list);
  const action = tablistKeys(list);
  return { list, tabs, extra, clicks, action };
}

const key = (el: HTMLElement, k: string) => {
  const e = new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true });
  el.dispatchEvent(e);
  return e;
};

describe('tablistKeys', () => {
  it('arrows move and select, wrapping; Home and End jump', () => {
    const { tabs, clicks } = strip();
    tabs[0].focus();
    expect(key(tabs[0], 'ArrowRight').defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(tabs[1]);
    key(tabs[1], 'End');
    key(tabs[2], 'ArrowRight');
    expect(document.activeElement).toBe(tabs[0]);
    key(tabs[0], 'ArrowLeft');
    key(tabs[2], 'Home');
    expect(clicks).toEqual([1, 2, 2]); // tab 0 is already selected
  });

  it('skips disabled tabs and leaves other keys and other buttons alone', () => {
    const { tabs, extra, clicks, action } = strip(0, [1]);
    key(tabs[0], 'ArrowRight');
    expect(document.activeElement).toBe(tabs[2]);
    expect(key(tabs[2], 'Enter').defaultPrevented).toBe(false);
    expect(key(extra, 'ArrowRight').defaultPrevented).toBe(false);
    action.destroy();
    expect(key(tabs[2], 'ArrowRight').defaultPrevented).toBe(false);
    expect(clicks).toEqual([2]);
  });

  it('does not click the tab that is already selected', () => {
    const { tabs, clicks } = strip(1);
    const spy = vi.fn();
    tabs[1].addEventListener('click', spy);
    key(tabs[0], 'ArrowRight');
    expect(spy).not.toHaveBeenCalled();
    expect(clicks).toEqual([]);
  });
});

// Serial files wait for their slot; each row is removed when its file takes
// `use:tablistKeys` (the test fails a row that no longer needs it).
const WAITING_FOR_SLOT: string[] = [];

describe('every tablist answers the arrow keys (review r11)', () => {
  it('is kit Tabs, handles the arrows itself, or uses tablistKeys', () => {
    const files = readdirSync('src', { recursive: true })
      .filter((n) => n.endsWith('.svelte'))
      .map((n) => `src/${n.replaceAll('\\', '/')}`);
    const bare = files.filter((f) => {
      const src = readFileSync(f, 'utf8');
      if (!/role="tablist"/.test(src)) return false;
      const lists = src.match(/<[a-z]+\b[^>]*role="tablist"[^>]*>/g) ?? [];
      const ownKeys = /ArrowRight/.test(src);
      return lists.some((tag) => !/use:tablistKeys/.test(tag)) && !ownKeys;
    });
    expect(bare).toEqual(WAITING_FOR_SLOT);
  });
});
