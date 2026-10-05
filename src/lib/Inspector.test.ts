import { render, screen, fireEvent } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { describe, it, expect, vi } from 'vitest';
import Inspector from './Inspector.svelte';

const tabs = [{ id: 'a', label: 'Alpha' }, { id: 'b', label: 'Beta' }, { id: 'c', label: 'Gamma' }];
const children = createRawSnippet(() => ({ render: () => '<p data-testid="body">body</p>' }));
const mount = (over: Record<string, unknown> = {}) => {
  const onchange = vi.fn();
  render(Inspector, { title: 'w', eyebrow: 'skill', tabs, active: 'a', onchange, children, ...over });
  return onchange;
};

describe('Inspector', () => {
  it('is a tablist with a labelled tabpanel; only the active tab is in the tab order', () => {
    mount();
    expect(screen.getByRole('tablist')).toBeTruthy();
    const [a, b] = screen.getAllByRole('tab');
    expect(a.getAttribute('aria-selected')).toBe('true');
    expect(a.getAttribute('tabindex')).toBe('0');
    expect(b.getAttribute('aria-selected')).toBe('false');
    expect(b.getAttribute('tabindex')).toBe('-1');
    const panel = screen.getByRole('tabpanel');
    expect(panel.getAttribute('aria-labelledby')).toBe(a.id);
    expect(a.getAttribute('aria-controls')).toBe(panel.id);
    expect(screen.getByTestId('body')).toBeTruthy();
    expect(screen.getByText('skill')).toBeTruthy();
  });

  it('a click picks a tab', async () => {
    const onchange = mount();
    await fireEvent.click(screen.getByTestId('inspector-tab-b'));
    expect(onchange).toHaveBeenCalledWith('b');
  });

  it('arrow keys wrap, Home and End jump', async () => {
    const onchange = mount();
    await fireEvent.keyDown(screen.getByTestId('inspector-tab-a'), { key: 'ArrowLeft' });
    expect(onchange).toHaveBeenLastCalledWith('c');
    await fireEvent.keyDown(screen.getByTestId('inspector-tab-a'), { key: 'ArrowRight' });
    expect(onchange).toHaveBeenLastCalledWith('b');
    await fireEvent.keyDown(screen.getByTestId('inspector-tab-a'), { key: 'End' });
    expect(onchange).toHaveBeenLastCalledWith('c');
    await fireEvent.keyDown(screen.getByTestId('inspector-tab-a'), { key: 'Home' });
    expect(onchange).toHaveBeenLastCalledWith('a');
  });

  it('other keys do nothing', async () => {
    const onchange = mount();
    await fireEvent.keyDown(screen.getByTestId('inspector-tab-a'), { key: 'x' });
    expect(onchange).not.toHaveBeenCalled();
  });
});
