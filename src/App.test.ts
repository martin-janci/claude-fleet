import { findByTestId, render } from '@testing-library/svelte';
import { fireEvent } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import { vi } from 'vitest';
import App from './App.svelte';

describe('App layout', () => {
  it('renders sidebar, center, and terminal panes', () => {
    const { getByTestId } = render(App);
    expect(getByTestId('pane-sidebar')).toBeInTheDocument();
    expect(getByTestId('pane-center')).toBeInTheDocument();
    expect(getByTestId('pane-terminal')).toBeInTheDocument();
  });

  it('contains all three panes inside the layout container', () => {
    const { container } = render(App);
    const layout = container.querySelector('.layout') as HTMLElement;
    expect(layout).not.toBeNull();
    const panes = layout.querySelectorAll('[data-testid^="pane-"]');
    expect(panes).toHaveLength(3);
  });

  it('mounts the sidebar tree inside the sidebar pane', async () => {
    const { container } = render(App);
    const sidebarTree = await findByTestId(container, 'sidebar-tree');
    expect(sidebarTree).toBeInTheDocument();
  });

  it('refreshes projects and sessions when the window regains focus', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    render(App);
    const before = (invoke as ReturnType<typeof vi.fn>).mock.calls.length;
    await fireEvent(window, new FocusEvent('focus'));
    const after = (invoke as ReturnType<typeof vi.fn>).mock.calls.length;
    expect(after).toBeGreaterThan(before);
    const cmds = (invoke as ReturnType<typeof vi.fn>).mock.calls.map((c) => c[0]);
    expect(cmds).toEqual(expect.arrayContaining(['list_projects', 'list_sessions']));
  });
});
