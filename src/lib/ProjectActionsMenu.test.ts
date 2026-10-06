import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { tick } from 'svelte';
import ProjectActionsMenu from './ProjectActionsMenu.svelte';

const base = { title: 'o/kuk-agent', pinned: false, hidden: false, groups: ['claude', 'openmarket', 'sales-twins'], currentGroup: null, manualGroup: false };

describe('ProjectActionsMenu', () => {
  it('main: Pin, Move to group…, Hide; Esc closes', async () => {
    const p = { ...base, startIn: 'main' as const, onpin: vi.fn(), onhide: vi.fn(), ongroup: vi.fn(), onclose: vi.fn() };
    render(ProjectActionsMenu, { props: p });
    await tick();
    expect(document.activeElement?.textContent).toContain('Pin to top');
    await fireEvent.click(screen.getByRole('menuitem', { name: /Hide/ }));
    expect(p.onhide).toHaveBeenCalled();
    await fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' });
    expect(p.onclose).toHaveBeenCalled();
  });

  it('groups: filter existing, create new, back to automatic', async () => {
    const p = { ...base, manualGroup: true, currentGroup: 'claude', startIn: 'groups' as const, onpin: vi.fn(), onhide: vi.fn(), ongroup: vi.fn(), onclose: vi.fn() };
    render(ProjectActionsMenu, { props: p });
    const input = screen.getByLabelText('Group name');
    await fireEvent.input(input, { target: { value: 'open' } });
    await tick();
    expect(screen.getAllByRole('menuitemradio').map((b) => b.textContent?.trim())).toEqual(['openmarket', 'New group “open”']);
    await fireEvent.click(screen.getByText('New group “open”'));
    expect(p.ongroup).toHaveBeenCalledWith('open');
    await fireEvent.input(input, { target: { value: '' } });
    await tick();
    await fireEvent.click(screen.getByText('Back to automatic'));
    expect(p.ongroup).toHaveBeenLastCalledWith(null);
  });
});
