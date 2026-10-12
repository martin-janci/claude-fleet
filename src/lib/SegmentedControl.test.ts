import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import SegmentedControl from './SegmentedControl.svelte';

const options = [
  { id: 'a', label: 'Alpha' },
  { id: 'b', label: 'Beta', title: 'The second one', testid: 'custom-b' },
  { id: 'c', label: 'Gamma' },
] as const;

function setup(value: 'a' | 'b' | 'c' = 'a') {
  const onchange = vi.fn();
  render(SegmentedControl, { props: { options, value, label: 'Pick one', testidPrefix: 'seg-', onchange } });
  return onchange;
}

describe('SegmentedControl', () => {
  it('is one labelled group of pressed-state buttons styled by controls.css', () => {
    setup('a');
    const group = screen.getByRole('group', { name: 'Pick one' });
    expect(group.classList.contains('seg-group')).toBe(true);
    expect(screen.getByTestId('seg-a').getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByTestId('custom-b').getAttribute('aria-pressed')).toBe('false');
    expect(screen.getByTestId('seg-c').getAttribute('aria-pressed')).toBe('false');
  });

  it('takes a per-option test id and title', () => {
    setup();
    const b = screen.getByTestId('custom-b');
    expect(b.textContent).toBe('Beta');
    expect(b.getAttribute('title')).toBe('The second one');
    expect(screen.queryByTestId('seg-b')).toBeNull();
  });

  it('reports a click as a choice', async () => {
    const onchange = setup('a');
    await fireEvent.click(screen.getByTestId('seg-c'));
    expect(onchange).toHaveBeenCalledWith('c', 'click');
  });

  it('arrows wrap both ways and move focus, reported as arrow', async () => {
    const onchange = setup('a');
    await fireEvent.keyDown(screen.getByTestId('seg-a'), { key: 'ArrowLeft' });
    expect(onchange).toHaveBeenLastCalledWith('c', 'arrow');
    expect(document.activeElement).toBe(screen.getByTestId('seg-c'));
    await fireEvent.keyDown(screen.getByTestId('seg-a'), { key: 'ArrowRight' });
    expect(onchange).toHaveBeenLastCalledWith('b', 'arrow');
    expect(document.activeElement).toBe(screen.getByTestId('custom-b'));
  });
});
