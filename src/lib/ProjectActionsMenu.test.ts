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

  const groupsProps = (over: Record<string, unknown> = {}) => ({
    ...base, startIn: 'groups' as const, onpin: vi.fn(), onhide: vi.fn(), ongroup: vi.fn(), onclose: vi.fn(), ...over,
  });

  it('Enter with an empty draft does nothing', async () => {
    const p = groupsProps({ manualGroup: true, currentGroup: 'claude' });
    render(ProjectActionsMenu, { props: p });
    await fireEvent.keyDown(screen.getByLabelText('Group name'), { key: 'Enter' });
    expect(p.ongroup).not.toHaveBeenCalled();
  });

  it('Enter creates the typed name unless it equals a group (any case)', async () => {
    const p = groupsProps({ groups: ['claude', 'openmarket'] });
    render(ProjectActionsMenu, { props: p });
    const input = screen.getByLabelText('Group name');
    await fireEvent.input(input, { target: { value: 'open' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(p.ongroup).toHaveBeenLastCalledWith('open');
    await fireEvent.input(input, { target: { value: 'OpenMarket' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(p.ongroup).toHaveBeenLastCalledWith('openmarket');
  });

  it('an exact name survives the six-row cap, ranked first', async () => {
    const many = ['a-x1', 'a-x2', 'a-x3', 'a-x4', 'a-x5', 'a-x6', 'x'];
    render(ProjectActionsMenu, { props: groupsProps({ groups: many }) });
    await fireEvent.input(screen.getByLabelText('Group name'), { target: { value: 'x' } });
    await tick();
    expect(screen.getAllByRole('menuitemradio')[0].textContent?.trim()).toBe('x');
  });

  it('↑/↓ cycle through the input and the items', async () => {
    render(ProjectActionsMenu, { props: groupsProps() });
    await tick();
    const input = screen.getByLabelText('Group name');
    expect(document.activeElement).toBe(input);
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    expect(document.activeElement?.textContent).toBe('claude');
    await fireEvent.keyDown(document.activeElement as Element, { key: 'ArrowUp' });
    expect(document.activeElement).toBe(input);
    // Up from the input wraps to the last item, Cancel (M15 G7.12).
    await fireEvent.keyDown(input, { key: 'ArrowUp' });
    expect(document.activeElement?.getAttribute('data-testid')).toBe('project-group-cancel');
    await fireEvent.keyDown(document.activeElement as Element, { key: 'ArrowUp' });
    expect(document.activeElement?.textContent).toBe('sales-twins');
  });

  it('each group says how many projects it holds, and Cancel closes (M15 G7.12)', async () => {
    const p = groupsProps({ counts: { claude: 3, openmarket: 1 } });
    render(ProjectActionsMenu, { props: p });
    await tick();
    expect(screen.getAllByTestId('project-group-count').map((e) => e.textContent)).toEqual(['3 projects', '1 project']);
    await fireEvent.click(screen.getByTestId('project-group-cancel'));
    expect(p.onclose).toHaveBeenCalled();
    expect(p.ongroup).not.toHaveBeenCalled();
  });

  it('the group input is capped at 40 characters', () => {
    render(ProjectActionsMenu, { props: groupsProps() });
    expect(screen.getByLabelText('Group name').getAttribute('maxlength')).toBe('40');
  });
});
