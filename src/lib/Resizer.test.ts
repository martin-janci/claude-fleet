import { fireEvent, render } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import Resizer from './Resizer.svelte';

describe('Resizer', () => {
  it('emits a "resize" event with the new pixel offset on pointer drag', async () => {
    const { getByTestId, container } = render(Resizer, { props: { id: 'a' } });
    const handle = getByTestId('resizer-a');

    let lastDelta: number | null = null;
    container.addEventListener('resize', (e: Event) => {
      lastDelta = (e as CustomEvent<number>).detail;
    });

    await fireEvent.pointerDown(handle, { clientX: 100, pointerId: 1 });
    await fireEvent.pointerMove(window, { clientX: 150, pointerId: 1 });
    await fireEvent.pointerUp(window, { clientX: 150, pointerId: 1 });

    expect(lastDelta).toBe(50);
  });
});
