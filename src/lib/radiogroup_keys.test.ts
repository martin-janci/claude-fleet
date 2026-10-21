// Review w19-8: a hand-built radio group has one Tab stop and arrow keys.
import { describe, it, expect } from 'vitest';
import { radiogroupKeys } from './radiogroup_keys';

function group(checked: number | null, opts?: { select?: boolean }) {
  const g = document.createElement('div');
  g.setAttribute('role', 'radiogroup');
  const clicks: number[] = [];
  const radios = [0, 1, 2].map((i) => {
    const b = document.createElement('button');
    b.setAttribute('role', 'radio');
    b.setAttribute('aria-checked', String(i === checked));
    b.addEventListener('click', () => {
      clicks.push(i);
      radios.forEach((r, j) => r.setAttribute('aria-checked', String(j === i)));
    });
    g.append(b);
    return b;
  });
  document.body.append(g);
  const action = radiogroupKeys(g, opts);
  return { g, radios, clicks, action };
}

const key = (el: HTMLElement, k: string) => {
  const e = new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true });
  el.dispatchEvent(e);
  return e;
};
const tabindexes = (rs: HTMLElement[]) => rs.map((r) => r.getAttribute('tabindex'));
const flush = () => new Promise((r) => setTimeout(r, 0));

describe('radiogroupKeys', () => {
  it('leaves one Tab stop: the checked radio, else the first', () => {
    expect(tabindexes(group(1).radios)).toEqual(['-1', '0', '-1']);
    expect(tabindexes(group(null).radios)).toEqual(['0', '-1', '-1']);
  });

  it('arrows, Home and End move, wrap and check', async () => {
    const { radios, clicks } = group(0);
    radios[0].focus();
    expect(key(radios[0], 'ArrowRight').defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(radios[1]);
    key(radios[1], 'ArrowDown');
    key(radios[2], 'ArrowRight');
    expect(document.activeElement).toBe(radios[0]);
    key(radios[0], 'ArrowUp');
    expect(document.activeElement).toBe(radios[2]);
    key(radios[2], 'Home');
    key(radios[0], 'End');
    expect(clicks).toEqual([1, 2, 0, 2, 0, 2]);
    await flush();
    expect(tabindexes(radios)).toEqual(['-1', '-1', '0']);
  });

  it('with select: false only moves focus', () => {
    const { radios, clicks } = group(0, { select: false });
    radios[0].focus();
    key(radios[0], 'ArrowRight');
    expect(document.activeElement).toBe(radios[1]);
    expect(clicks).toEqual([]);
    expect(tabindexes(radios)).toEqual(['-1', '0', '-1']);
  });
});
