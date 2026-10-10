// The destructive confirm sheet (G1.4, FormsAnatomy "Destructive confirm"):
// a red verb, a safer way out on the left, and the typed name only when
// the loss is large.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import DestructiveConfirm from './DestructiveConfirm.svelte';

function sheet(props: Record<string, unknown> = {}) {
  const onconfirm = vi.fn();
  const onclose = vi.fn();
  render(DestructiveConfirm, {
    props: {
      title: 'Delete routine "Morning PR sweep"?',
      lead: "Its 41 runs go with it. This can't be undone.",
      verb: 'Delete routine',
      name: 'Morning PR sweep',
      noun: 'routine',
      onconfirm,
      onclose,
      ...props,
    },
  });
  return { onconfirm, onclose, go: () => screen.getByTestId('destructive-confirm-go') as HTMLButtonElement };
}

describe('DestructiveConfirm', () => {
  it('a small loss asks with the red verb alone: no name to type', async () => {
    const { onconfirm, go } = sheet({ loss: 2 });
    expect(screen.queryByTestId('destructive-typed-name')).toBeNull();
    expect(go().classList.contains('btn--danger')).toBe(true);
    expect(go().textContent).toBe('Delete routine');
    expect(go().disabled).toBe(false);
    await fireEvent.click(go());
    expect(onconfirm).toHaveBeenCalledOnce();
  });

  it('a large loss keeps the verb off, saying why under it, until the name is typed', async () => {
    const { onconfirm, go } = sheet({ loss: 41 });
    const input = screen.getByTestId('destructive-typed-name') as HTMLInputElement;
    expect(screen.getByText('Type the routine name to confirm')).toBeTruthy();
    expect(go().disabled).toBe(true);
    expect(screen.getByTestId('sheet-why').textContent).toBe('Type the routine name to confirm.');
    await fireEvent.input(input, { target: { value: 'Morning PR' } });
    expect(go().disabled).toBe(true);
    await fireEvent.input(input, { target: { value: 'Morning PR sweep' } });
    expect(go().disabled).toBe(false);
    expect(screen.queryByTestId('sheet-why')).toBeNull();
    await fireEvent.click(go());
    expect(onconfirm).toHaveBeenCalledOnce();
  });

  it('Enter in the typed name confirms only once it matches', async () => {
    const { onconfirm } = sheet({ loss: 41 });
    const input = screen.getByTestId('destructive-typed-name') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'wrong' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(onconfirm).not.toHaveBeenCalled();
    await fireEvent.input(input, { target: { value: 'Morning PR sweep' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(onconfirm).toHaveBeenCalledOnce();
  });

  it('puts the safer way out on the left, before Cancel and the verb, and runs it instead', async () => {
    const run = vi.fn();
    const { onconfirm, go } = sheet({ loss: 41, safer: { label: 'Pause it instead', run } });
    const safer = screen.getByTestId('destructive-safer');
    expect(safer.textContent).toBe('Pause it instead');
    const cancel = screen.getByTestId('sheet-cancel');
    expect(safer.compareDocumentPosition(cancel) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(cancel.compareDocumentPosition(go()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await fireEvent.click(safer);
    expect(run).toHaveBeenCalledOnce();
    expect(onconfirm).not.toHaveBeenCalled();
  });

  it('shows no safer way out when there is none, and a failure as the banner on top', () => {
    sheet({ error: 'The routine is running a fire.' });
    expect(screen.queryByTestId('destructive-safer')).toBeNull();
    const banner = screen.getByTestId('destructive-confirm-error');
    expect(banner.textContent).toMatch(/running a fire/);
    expect(banner.compareDocumentPosition(screen.getByTestId('sheet-cancel')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});
