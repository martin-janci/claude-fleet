import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import Lazy, { preload } from './Lazy.svelte';
import Probe from './__fixtures__/LazyProbe.svelte';

describe('Lazy', () => {
  it('renders the loaded view with its props, importing it once', async () => {
    const load = vi.fn(() => Promise.resolve({ default: Probe }));
    render(Lazy, { load, label: 'hello' });
    await vi.waitFor(() => expect(screen.getByText('hello')).toBeTruthy());
    render(Lazy, { load, label: 'again' });
    await tick();
    expect(screen.getByText('again')).toBeTruthy();
    await preload(load);
    expect(load).toHaveBeenCalledTimes(1);
  });
});
