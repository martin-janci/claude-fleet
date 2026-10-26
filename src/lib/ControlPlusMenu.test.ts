// The composer's "+" menu (review w19-6/7): a real menu — focus moves to the
// first item, arrows/Home/End move, Escape or a pick gives focus back to "+",
// and the group headings sit in named groups, not directly in the menu.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));
import ControlPlusMenu from './ControlPlusMenu.svelte';
import { expectAccessible } from './a11y_check';

const settle = async () => {
  await tick();
  await tick();
};

describe('ControlPlusMenu', () => {
  it('focuses the first item, moves with arrows, and passes axe', async () => {
    const { container } = render(ControlPlusMenu, { sessionId: 1, host: 'mac' });
    await fireEvent.click(screen.getByTestId('control-plus'));
    await settle();
    expect(document.activeElement).toBe(screen.getByTestId('control-plus-project'));
    await fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(screen.getByTestId('control-plus-session'));
    await fireEvent.keyDown(document.activeElement!, { key: 'End' });
    expect(document.activeElement).toBe(screen.getByTestId('control-plus-graph'));
    await fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(screen.getByTestId('control-plus-project'));
    await fireEvent.keyDown(document.activeElement!, { key: 'ArrowUp' });
    expect(document.activeElement).toBe(screen.getByTestId('control-plus-graph'));
    expect(screen.getByRole('menu').querySelectorAll(':scope > p').length).toBe(0);
    expect(screen.getAllByRole('group').map((g) => g.getAttribute('aria-labelledby') && document.getElementById(g.getAttribute('aria-labelledby')!)?.textContent)).toEqual([
      'Add',
      'Ask Control',
    ]);
    await expectAccessible(container);
  });

  it('Escape and a pick give focus back to "+"', async () => {
    render(ControlPlusMenu, { sessionId: 1, host: 'mac' });
    const plus = screen.getByTestId('control-plus');
    await fireEvent.click(plus);
    await settle();
    await fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    expect(screen.queryByTestId('control-plus-menu')).toBeNull();
    expect(document.activeElement).toBe(plus);
    await fireEvent.click(plus);
    await settle();
    await fireEvent.click(screen.getByTestId('control-plus-task'));
    expect(screen.queryByTestId('control-plus-menu')).toBeNull();
    expect(document.activeElement).toBe(plus);
  });
});
