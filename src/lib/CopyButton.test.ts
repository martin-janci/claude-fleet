import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

vi.mock('./clipboard', async () => {
  const actual = await vi.importActual<typeof import('./clipboard')>('./clipboard');
  return { ...actual, copyText: vi.fn() };
});
import { copyText } from './clipboard';
import CopyButton from './CopyButton.svelte';

const mockedCopy = copyText as unknown as ReturnType<typeof vi.fn>;

async function settle() {
  await tick();
  await Promise.resolve();
  await tick();
}

beforeEach(() => {
  mockedCopy.mockReset();
});

describe('CopyButton', () => {
  it('names what it copies, and its label follows the state', async () => {
    mockedCopy.mockResolvedValue(true);
    render(CopyButton, { text: 'hi', label: 'Copy prompt' });
    const btn = screen.getByTestId('conv-copy');
    expect(btn.getAttribute('aria-label')).toBe('Copy prompt');
    expect(btn.textContent).toBe('Copy');
    await fireEvent.click(btn);
    await settle();
    expect(mockedCopy).toHaveBeenCalledWith('hi');
    expect(btn.getAttribute('aria-label')).toBe('Copied');
    expect(btn.textContent).toBe('Copied');
    expect(btn.getAttribute('title')).toBe('Copied');
  });

  it('defaults its label to "Copy"', () => {
    render(CopyButton, { text: 'hi' });
    expect(screen.getByTestId('conv-copy').getAttribute('aria-label')).toBe('Copy');
  });

  it('a failed copy keeps the idle label', async () => {
    mockedCopy.mockResolvedValue(false);
    render(CopyButton, { text: 'hi', label: 'Copy reply' });
    await fireEvent.click(screen.getByTestId('conv-copy'));
    await settle();
    expect(screen.getByTestId('conv-copy').getAttribute('aria-label')).toBe('Copy reply');
  });

  it('a copied-note is shown in the title after copying', async () => {
    mockedCopy.mockResolvedValue(true);
    render(CopyButton, { text: 'x', label: 'Copy result', copiedNote: 'truncated at 8 000 chars' });
    const btn = screen.getByTestId('conv-copy');
    await fireEvent.click(btn);
    await settle();
    expect(btn.getAttribute('title')).toBe('Copied (truncated at 8 000 chars)');
  });
});
