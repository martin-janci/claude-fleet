import { describe, expect, it } from 'vitest';
import { canStop, cancelInto, drop, due, heldPreview, hold, STOP_KEY } from './composer_hold';

describe('composer_hold', () => {
  it('holds lines in order and sends the first once the gate opens', () => {
    let held = hold([], 'one');
    held = hold(held, '  ');
    held = hold(held, 'two');
    expect(held.map((h) => h.text)).toEqual(['one', 'two']);
    expect(due(held, true, false)).toBeNull();
    expect(due(held, false, true)).toBeNull();
    expect(due(held, false, false)?.text).toBe('one');
    expect(due([], false, false)).toBeNull();
  });

  it('cancel drops the line and gives its text back to the box', () => {
    const held = hold(hold([], 'one'), 'two');
    const [first] = held;
    expect(drop(held, first.id).map((h) => h.text)).toEqual(['two']);
    expect(cancelInto('', first)).toBe('one');
    expect(cancelInto('started', first)).toBe('one\n\nstarted');
  });

  it('previews the first line, cut short', () => {
    expect(heldPreview('\n  hello there \nmore')).toBe('hello there');
    const p = heldPreview('x'.repeat(100), 20);
    expect(p).toHaveLength(20);
    expect(p.endsWith('…')).toBe(true);
  });

  it('Stop shows on a gated composer while the session is busy, and presses Escape', () => {
    expect(canStop(true, 'Claude is working')).toBe(true);
    expect(canStop(true, null)).toBe(false);
    expect(canStop(false, 'Claude is working')).toBe(false);
    expect(STOP_KEY).toBe('Escape');
  });
});
