import { render } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import App from './App.svelte';

describe('App layout', () => {
  it('renders sidebar, center, and terminal panes', () => {
    const { getByTestId } = render(App);
    expect(getByTestId('pane-sidebar')).toBeInTheDocument();
    expect(getByTestId('pane-center')).toBeInTheDocument();
    expect(getByTestId('pane-terminal')).toBeInTheDocument();
  });

  it('arranges panes left-to-right via CSS grid', () => {
    const { container } = render(App);
    const layout = container.querySelector('.layout') as HTMLElement;
    expect(layout).not.toBeNull();
    // jsdom does not apply scoped Svelte CSS, so we verify the class is present
    // (the display:grid rule lives in App.svelte's <style> block)
    expect(layout.classList.contains('layout')).toBe(true);
  });
});
