import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import ConfirmDialog from './ConfirmDialog.svelte';
import PromptDialog from './PromptDialog.svelte';

beforeEach(() => {
  document.body.innerHTML = '';
});

describe('Modal (via ConfirmDialog)', () => {
  it('renders a native <dialog> opened as modal with an accessible name', async () => {
    render(ConfirmDialog, {
      props: { title: 'Kill session?', message: 'Sure?', onconfirm: () => {}, oncancel: () => {} },
    });
    await tick();
    const dlg = screen.getByRole('dialog', { name: 'Kill session?' }) as HTMLDialogElement;
    expect(dlg.tagName).toBe('DIALOG');
    expect(dlg.hasAttribute('open')).toBe(true);
    expect(dlg.getAttribute('aria-modal')).toBe('true');
  });

  it('Escape (the dialog cancel event) calls oncancel and does not close the element itself', async () => {
    const oncancel = vi.fn();
    render(ConfirmDialog, { props: { title: 'T', message: 'm', onconfirm: () => {}, oncancel } });
    await tick();
    const dlg = screen.getByRole('dialog') as HTMLDialogElement;
    const ev = new Event('cancel', { cancelable: true });
    dlg.dispatchEvent(ev);
    expect(oncancel).toHaveBeenCalledTimes(1);
    expect(ev.defaultPrevented).toBe(true);
    expect(dlg.hasAttribute('open')).toBe(true); // parent's {#if} owns visibility
  });

  it('a backdrop click (target is the <dialog>) calls oncancel; a click inside does not', async () => {
    const oncancel = vi.fn();
    render(ConfirmDialog, { props: { title: 'T', message: 'm', onconfirm: () => {}, oncancel } });
    await tick();
    const dlg = screen.getByRole('dialog');
    await fireEvent.click(screen.getByText('m'));
    expect(oncancel).not.toHaveBeenCalled();
    await fireEvent.click(dlg);
    expect(oncancel).toHaveBeenCalledTimes(1);
  });

  it('confirm and cancel buttons call their handlers', async () => {
    const onconfirm = vi.fn();
    const oncancel = vi.fn();
    render(ConfirmDialog, {
      props: { title: 'T', message: 'm', confirmLabel: 'Kill', danger: true, onconfirm, oncancel, confirmTestId: 'go' },
    });
    await tick();
    await fireEvent.click(screen.getByTestId('go'));
    expect(onconfirm).toHaveBeenCalledTimes(1);
    await fireEvent.click(screen.getByTestId('confirm-cancel'));
    expect(oncancel).toHaveBeenCalledTimes(1);
  });

  it('a danger confirm focuses Cancel initially; a plain one focuses the confirm button', async () => {
    const a = render(ConfirmDialog, {
      props: { title: 'T', message: 'm', danger: true, onconfirm: () => {}, oncancel: () => {}, confirmTestId: 'go' },
    });
    await tick();
    expect(document.activeElement).toBe(screen.getByTestId('confirm-cancel'));
    a.unmount();
    render(ConfirmDialog, {
      props: { title: 'T', message: 'm', onconfirm: () => {}, oncancel: () => {}, confirmTestId: 'go' },
    });
    await tick();
    expect(document.activeElement).toBe(screen.getByTestId('go'));
  });

  it('restores focus to the previously focused element on unmount', async () => {
    const opener = document.createElement('button');
    opener.textContent = 'open';
    document.body.appendChild(opener);
    opener.focus();
    expect(document.activeElement).toBe(opener);
    const r = render(ConfirmDialog, {
      props: { title: 'T', message: 'm', onconfirm: () => {}, oncancel: () => {} },
    });
    await tick();
    expect(document.activeElement).not.toBe(opener);
    r.unmount();
    expect(document.activeElement).toBe(opener);
  });

  it('does not call onclose again while tearing down (native close during unmount)', async () => {
    const oncancel = vi.fn();
    const r = render(ConfirmDialog, { props: { title: 'T', message: 'm', onconfirm: () => {}, oncancel } });
    await tick();
    r.unmount();
    expect(oncancel).not.toHaveBeenCalled();
  });
});

describe('PromptDialog', () => {
  it('submits the trimmed value on Enter / submit and disables submit while empty', async () => {
    const onsubmit = vi.fn();
    render(PromptDialog, {
      props: { title: 'New branch', label: 'Branch name', onsubmit, oncancel: () => {} },
    });
    await tick();
    const input = screen.getByTestId('prompt-input') as HTMLInputElement;
    const submit = screen.getByTestId('prompt-submit') as HTMLButtonElement;
    expect(document.activeElement).toBe(input);
    expect(submit.disabled).toBe(true);
    await fireEvent.input(input, { target: { value: '  feat/x  ' } });
    await tick();
    expect(submit.disabled).toBe(false);
    await fireEvent.submit(input.closest('form')!);
    expect(onsubmit).toHaveBeenCalledWith('feat/x');
  });

  it('shows the validate() error and blocks submit', async () => {
    const onsubmit = vi.fn();
    render(PromptDialog, {
      props: {
        title: 'New branch',
        label: 'Branch name',
        validate: (v: string) => (v.includes('..') ? 'no dots' : null),
        onsubmit,
        oncancel: () => {},
      },
    });
    await tick();
    const input = screen.getByTestId('prompt-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'a..b' } });
    await tick();
    expect(screen.getByTestId('prompt-error').textContent).toBe('no dots');
    expect((screen.getByTestId('prompt-submit') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.submit(input.closest('form')!);
    expect(onsubmit).not.toHaveBeenCalled();
  });

  it('seeds the input with initialValue', async () => {
    render(PromptDialog, {
      props: { title: 'Rename', label: 'Name', initialValue: 'dev-foo', onsubmit: () => {}, oncancel: () => {} },
    });
    await tick();
    expect((screen.getByTestId('prompt-input') as HTMLInputElement).value).toBe('dev-foo');
  });
});
