import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { createRawSnippet } from 'svelte';
import PickerList from './PickerList.svelte';

const acts = createRawSnippet(() => ({ render: () => '<button type="button" tabindex="-1" data-testid="act">x</button>' }));

describe('PickerList extensions', () => {
  it('renders chip, kbd, dim, a group subtitle, and mouse-only actions', async () => {
    const ongroupclick = vi.fn();
    const oncontext = vi.fn();
    const onpick = vi.fn();
    render(PickerList, {
      props: {
        items: [
          { key: 'a', label: 'alpha', group: 'Pinned', groupKey: 'pinned', groupSub: 'yours', chip: 'current session', kbd: '⌘1', actionable: true },
          { key: 'b', label: 'beta', group: 'Pinned', groupKey: 'pinned', dim: true },
        ],
        onpick,
        rowActions: acts,
        ongroupclick,
        oncontext,
        listId: 'l',
      },
    });
    expect(screen.getByText('current session')).toBeTruthy();
    expect(screen.getByText('⌘1')).toBeTruthy();
    expect(screen.getByText('yours')).toBeTruthy();
    expect(document.querySelector('[data-key="b"]')?.classList.contains('dim')).toBe(true);
    expect(screen.getByTestId('act').closest('[aria-hidden="true"]')).not.toBeNull();
    expect(screen.getAllByTestId('act')).toHaveLength(1); // only the actionable row
    expect(document.querySelector('[data-key="a"]')?.classList.contains('actionable')).toBe(true);
    expect(document.querySelector('[data-key="b"]')?.classList.contains('actionable')).toBe(false);
    await fireEvent.click(screen.getByTestId('act'));
    expect(onpick).not.toHaveBeenCalled();
    await fireEvent.keyDown(screen.getByTestId('act'), { key: 'Enter' });
    expect(onpick).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByText('Pinned'));
    expect(ongroupclick).toHaveBeenCalledWith('pinned');
    await fireEvent.contextMenu(document.querySelector('[data-key="a"]')!);
    expect(oncontext).toHaveBeenCalledWith('a', expect.anything());
  });
});
