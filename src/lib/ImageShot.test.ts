// An inline screenshot and its full-size lightbox (review w19-5): focus
// moves into the layer, Tab stays there, and closing returns focus to the
// shot so a long transcript keeps your place.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import { tick } from 'svelte';
import ImageShot from './ImageShot.svelte';

const settle = async () => {
  await tick();
  await Promise.resolve();
};

describe('ImageShot', () => {
  for (const how of ['Escape', 'Close', 'backdrop'] as const) {
    it(`gives focus back to the shot after ${how}`, async () => {
      render(ImageShot, { mime: 'image/png', data: 'AAAA', alt: 'screen' });
      const shot = screen.getByTestId('image-shot');
      shot.focus();
      await fireEvent.click(shot);
      await settle();
      const close = screen.getByTestId('image-shot-close');
      expect(document.activeElement).toBe(close);
      await fireEvent.keyDown(close, { key: 'Tab', shiftKey: true });
      expect(document.activeElement).toBe(close);
      const layer = screen.getByTestId('image-shot-full');
      if (how === 'Escape') await fireEvent.keyDown(close, { key: 'Escape' });
      else if (how === 'Close') await fireEvent.click(close);
      else await fireEvent.click(layer);
      await settle();
      expect(screen.queryByTestId('image-shot-full')).toBeNull();
      expect(document.activeElement).toBe(shot);
    });
  }
});
