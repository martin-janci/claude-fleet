// The shared inline confirm: Keep holds focus as it mounts, Escape is Keep,
// and focus goes back to the opener when the row closes.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, afterEach } from 'vitest';
import InlineConfirm from './InlineConfirm.svelte';

function row(props: Record<string, unknown> = {}) {
  const onconfirm = vi.fn();
  const onkeep = vi.fn();
  const r = render(InlineConfirm, {
    props: { question: 'Delete it?', verb: 'Delete', onconfirm, onkeep, testid: 'ic', confirmTestid: 'ic-go', keepTestid: 'ic-keep', ...props },
  });
  return { ...r, onconfirm, onkeep };
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('InlineConfirm', () => {
  it('asks with a red verb and lands focus on Keep', async () => {
    const { onconfirm } = row();
    const el = screen.getByTestId('ic');
    expect(el.getAttribute('role')).toBe('alertdialog');
    expect(el.getAttribute('aria-label')).toBe('Delete it?');
    expect(el.textContent).toContain('Delete it?');
    expect(screen.getByTestId('ic-go').classList.contains('btn--crit')).toBe(true);
    expect(document.activeElement).toBe(screen.getByTestId('ic-keep'));
    await fireEvent.click(screen.getByTestId('ic-go'));
    expect(onconfirm).toHaveBeenCalledOnce();
  });

  it('Escape is Keep, and does not reach the dialog around it', async () => {
    const outer = vi.fn();
    document.addEventListener('keydown', outer);
    const { onkeep, onconfirm } = row();
    await fireEvent.keyDown(screen.getByTestId('ic-keep'), { key: 'Escape' });
    expect(onkeep).toHaveBeenCalledOnce();
    expect(onconfirm).not.toHaveBeenCalled();
    expect(outer).not.toHaveBeenCalled();
    document.removeEventListener('keydown', outer);
  });

  it('a busy row turns both buttons off; a non-danger verb is not red', () => {
    row({ busy: true, danger: false, confirmTitle: 'Working' });
    const go = screen.getByTestId('ic-go') as HTMLButtonElement;
    expect(go.disabled).toBe(true);
    expect(go.title).toBe('Working');
    expect(go.classList.contains('btn--crit')).toBe(false);
    expect((screen.getByTestId('ic-keep') as HTMLButtonElement).disabled).toBe(true);
  });

  it('closing with focus inside returns it to the element focused before it opened', async () => {
    const opener = document.createElement('button');
    opener.textContent = 'Delete…';
    document.body.appendChild(opener);
    opener.focus();
    const { unmount } = row();
    expect(document.activeElement).toBe(screen.getByTestId('ic-keep'));
    unmount();
    await Promise.resolve();
    expect(document.activeElement).toBe(opener);
  });

  it('an opener given wins over the captured one', async () => {
    const other = document.createElement('button');
    document.body.appendChild(other);
    const { unmount } = row({ opener: () => other });
    unmount();
    await Promise.resolve();
    expect(document.activeElement).toBe(other);
  });
});
