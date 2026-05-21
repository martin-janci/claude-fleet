import { fireEvent, render } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import Resizer from './Resizer.svelte';

describe('Resizer', () => {
  it('emits the per-frame delta on a single pointer move', async () => {
    const onresize = vi.fn<(delta: number) => void>();
    const { getByTestId } = render(Resizer, { props: { id: 'a', onresize } });
    const handle = getByTestId('resizer-a');

    await fireEvent.pointerDown(handle, { clientX: 100, pointerId: 1 });
    await fireEvent.pointerMove(handle, { clientX: 150, pointerId: 1 });
    await fireEvent.pointerUp(handle, { clientX: 150, pointerId: 1 });

    expect(onresize).toHaveBeenCalledTimes(1);
    expect(onresize).toHaveBeenLastCalledWith(50);
  });

  it('emits per-frame deltas (not cumulative) across multiple moves', async () => {
    const onresize = vi.fn<(delta: number) => void>();
    const { getByTestId } = render(Resizer, { props: { id: 'b', onresize } });
    const handle = getByTestId('resizer-b');

    await fireEvent.pointerDown(handle, { clientX: 100, pointerId: 1 });
    await fireEvent.pointerMove(handle, { clientX: 130, pointerId: 1 });
    await fireEvent.pointerMove(handle, { clientX: 150, pointerId: 1 });
    await fireEvent.pointerUp(handle, { clientX: 150, pointerId: 1 });

    expect(onresize).toHaveBeenCalledTimes(2);
    expect(onresize).toHaveBeenNthCalledWith(1, 30);
    expect(onresize).toHaveBeenNthCalledWith(2, 20);
  });
});
