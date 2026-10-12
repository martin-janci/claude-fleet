import { describe, expect, it } from 'vitest';
import { windowHidden } from './window_hidden';

describe('windowHidden', () => {
  it('is true only for a hidden document', () => {
    expect(windowHidden({ visibilityState: 'hidden' })).toBe(true);
    expect(windowHidden({ visibilityState: 'visible' })).toBe(false);
    expect(windowHidden(null)).toBe(false);
  });
});
