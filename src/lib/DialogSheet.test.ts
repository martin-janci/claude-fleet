// DialogSheet carries the form kit (G1.2, FormsAnatomy) for every sheet:
// the failure banner on top, the reason under an off verb, the submit keys,
// and "Discard changes?" once when a changed sheet is closed.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { createRawSnippet, tick } from 'svelte';
import DialogSheet from './DialogSheet.svelte';

const oneField = createRawSnippet(() => ({ render: () => '<input type="text" data-testid="f1">' }));
const twoFields = createRawSnippet(() => ({
  render: () => '<div><input type="text" data-testid="f1"><textarea data-testid="f2"></textarea></div>',
}));

function sheet(props: Record<string, unknown> = {}) {
  const onconfirm = vi.fn();
  const onclose = vi.fn();
  render(DialogSheet, {
    props: { title: 'Rename', lead: 'One sentence.', verb: 'Rename', onconfirm, onclose, children: oneField, confirmTestid: 'go', ...props },
  });
  return { onconfirm, onclose };
}

describe('DialogSheet: form kit', () => {
  it('puts a failure in a banner at the top of the body, above the fields', () => {
    sheet({ error: { code: 'E_FORBIDDEN', message: 'you are a Viewer in 32bit' }, errorTestid: 'err' });
    const banner = screen.getByTestId('err');
    expect(banner.textContent).toMatch(/The hub refused this: you are a Viewer in 32bit\./);
    expect(banner.textContent).toMatch(/Your changes are kept\. Ask an admin/);
    expect(banner.getAttribute('data-kind')).toBe('refused');
    // The banner comes before the field in the document.
    const field = screen.getByTestId('f1');
    expect(banner.compareDocumentPosition(field) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('says why the verb is off under it, and says nothing once it is on', async () => {
    const { rerender } = render(DialogSheet, {
      props: {
        title: 'T',
        lead: 'L',
        verb: 'Go',
        onconfirm: () => {},
        onclose: () => {},
        children: oneField,
        canConfirm: false,
        confirmTitle: 'The hub is offline.',
      },
    });
    expect(screen.getByTestId('sheet-why').textContent).toBe('The hub is offline.');
    await rerender({ canConfirm: true });
    expect(screen.queryByTestId('sheet-why')).toBeNull();
  });

  it('Enter in the one field submits', async () => {
    const { onconfirm } = sheet();
    await fireEvent.keyDown(screen.getByTestId('f1'), { key: 'Enter' });
    expect(onconfirm).toHaveBeenCalledOnce();
  });

  it('Enter does not submit a two-field sheet; Ctrl+Enter does, from the textarea too', async () => {
    const { onconfirm } = sheet({ children: twoFields });
    await fireEvent.keyDown(screen.getByTestId('f1'), { key: 'Enter' });
    expect(onconfirm).not.toHaveBeenCalled();
    await fireEvent.keyDown(screen.getByTestId('f2'), { key: 'Enter', ctrlKey: true });
    expect(onconfirm).toHaveBeenCalledOnce();
  });

  it('a submit key on an off verb reports the attempt instead of confirming', async () => {
    const oninvalid = vi.fn();
    const { onconfirm } = sheet({ canConfirm: false, oninvalid });
    await fireEvent.keyDown(screen.getByTestId('f1'), { key: 'Enter' });
    expect(onconfirm).not.toHaveBeenCalled();
    expect(oninvalid).toHaveBeenCalledOnce();
  });

  it('closing an untouched sheet closes at once', async () => {
    const { onclose } = sheet();
    await fireEvent.click(screen.getByTestId('sheet-cancel'));
    expect(onclose).toHaveBeenCalledOnce();
  });

  it('closing a changed sheet asks "Discard changes?" once, in the footer', async () => {
    const { onclose, onconfirm } = sheet({ dirty: true });
    await fireEvent.click(screen.getByTestId('sheet-cancel'));
    expect(onclose).not.toHaveBeenCalled();
    expect(screen.getByTestId('form-discard-ask').textContent).toMatch(/Discard changes\?/);
    // The footer's verb is replaced while it asks; Enter does not submit.
    expect(screen.queryByTestId('go')).toBeNull();
    await fireEvent.keyDown(screen.getByTestId('f1'), { key: 'Enter' });
    expect(onconfirm).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByTestId('form-keep-editing'));
    expect(screen.queryByTestId('form-discard-ask')).toBeNull();
    expect(screen.getByTestId('go')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('sheet-cancel'));
    await fireEvent.click(screen.getByTestId('form-discard'));
    expect(onclose).toHaveBeenCalledOnce();
  });

  it('Escape asks first, and a second Escape closes', async () => {
    const { onclose } = sheet({ dirty: true });
    const dialog = document.querySelector('dialog')!;
    await fireEvent(dialog, new Event('cancel', { cancelable: true }));
    await tick();
    expect(onclose).not.toHaveBeenCalled();
    expect(screen.getByTestId('form-discard-ask')).toBeTruthy();
    await fireEvent(dialog, new Event('cancel', { cancelable: true }));
    expect(onclose).toHaveBeenCalledOnce();
  });
});
