import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { flushSync } from 'svelte';
import ListLoading from './ListLoading.svelte';

afterEach(() => vi.useRealTimers());

// Redesign step 3.13: a loading list shows the Dot wave with its words.
describe('ListLoading', () => {
  it('says what it waits for at once and shows the Dot wave after 400 ms', () => {
    vi.useFakeTimers();
    render(ListLoading, { props: { text: 'Loading 48 rows from 4 hosts' } });
    const box = screen.getByTestId('list-loading');
    expect(box).toHaveAttribute('role', 'status');
    expect(box).toHaveTextContent('Loading 48 rows from 4 hosts');
    expect(screen.queryByTestId('loader')).toBeNull();
    vi.advanceTimersByTime(400);
    flushSync();
    expect(screen.getByTestId('loader').dataset.loader).toBe('dot-wave');
  });

  it('defaults to “Loading…”', () => {
    render(ListLoading);
    expect(screen.getByText('Loading…')).toBeInTheDocument();
  });
});
