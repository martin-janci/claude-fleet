import { describe, it, expect, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { tick } from 'svelte';
import { initTextSize, textSize } from './text_size';

afterEach(() => {
  textSize.set(100);
  document.documentElement.style.removeProperty('zoom');
});

// Review r4 w04-5: w14-2's fix (a pop-out follows the main window's text
// size) had no test; dropping `followPref` from `initTextSize` passed CI.
describe('text size across windows', () => {
  it('follows a size picked in another window, on <html> too', async () => {
    const stop = initTextSize();
    try {
      window.dispatchEvent(new StorageEvent('storage', { key: 'cf:pref:ui.textSize', newValue: '125' }));
      expect(get(textSize)).toBe(125);
      // Outside the desktop app the size is CSS zoom on <html>.
      for (let i = 0; i < 5; i++) await tick();
      expect(document.documentElement.style.getPropertyValue('zoom')).toBe('1.25');
      // The store's own subscriber writes it back: the same value, no loop.
      expect(localStorage.getItem('cf:pref:ui.textSize')).toBe('125');
      // Not one of the sizes: ignored.
      window.dispatchEvent(new StorageEvent('storage', { key: 'cf:pref:ui.textSize', newValue: '333' }));
      expect(get(textSize)).toBe(125);
    } finally {
      stop();
    }
  });
});
