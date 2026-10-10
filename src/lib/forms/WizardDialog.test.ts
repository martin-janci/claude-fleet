import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, afterEach } from 'vitest';
import WizardDialog from './WizardDialog.svelte';
import type { Wizard } from './wizards';

// Review r12 (loaders): one loader per screen. While a wizard's last step
// runs, its own loader sits in the body and the button only says what it is
// doing; the Comet stays for a form with no flow loader (ChatForm).

afterEach(() => vi.useRealTimers());

const wizard: Wizard = {
  id: 'add_project',
  sending: 'Adding…',
  loader: 'sonar',
  spec: {
    format: 'fleet.form/1',
    title: 'Add a project',
    submit: 'Add',
    steps: [{ title: 'Project', fields: [{ name: 'name', label: 'Name', type: 'text', required: false }] }],
  },
} as unknown as Wizard;

describe('WizardDialog while it runs', () => {
  it('shows the wizard’s own loader and no Comet in the button', async () => {
    vi.useFakeTimers();
    render(WizardDialog, { wizard, busy: true, run: () => {}, onclose: () => {} });
    await vi.advanceTimersByTimeAsync(400);
    const running = screen.getByTestId('wizard-running');
    expect(running.querySelector('[data-loader="sonar"]')).not.toBeNull();
    expect(document.querySelectorAll('[data-loader]')).toHaveLength(1);
    expect(document.querySelector('[data-loader="comet"]')).toBeNull();
  });
});

// G1.2 (FormsAnatomy): the kit's behaviour in a wizard dialog.
const required: Wizard = {
  ...wizard,
  spec: {
    ...wizard.spec,
    steps: [
      { title: 'Project', fields: [{ name: 'name', label: 'Name', type: 'text', required: true }] },
      { title: 'More', fields: [{ name: 'repo', label: 'Repo', type: 'text', required: false }, { name: 'note', label: 'Note', type: 'textarea', required: false }] },
    ],
  },
} as unknown as Wizard;

describe('WizardDialog: form kit', () => {
  it('checks a field when the person leaves it, not while they type', async () => {
    render(WizardDialog, { wizard: required, run: () => {}, onclose: () => {} });
    const name = screen.getByTestId('form-field-name');
    // The off Next says why under it from the start.
    expect(screen.getByTestId('form-submit-why').textContent).toBe('Name is required.');
    await fireEvent.input(name, { target: { value: '' } });
    expect(screen.queryByTestId('form-problem-name')).toBeNull();
    await fireEvent.focusOut(name);
    expect(screen.getByTestId('form-problem-name').textContent).toBe('is required');
    await fireEvent.input(name, { target: { value: 'pos' } });
    expect(screen.queryByTestId('form-problem-name')).toBeNull();
    expect(screen.queryByTestId('form-submit-why')).toBeNull();
  });

  it('Enter goes on from a one-field step; Ctrl+Enter submits the last', async () => {
    const run = vi.fn();
    render(WizardDialog, { wizard: required, run, onclose: () => {} });
    // Unfinished: Enter shows the problem instead of going on.
    await fireEvent.keyDown(screen.getByTestId('form-field-name'), { key: 'Enter' });
    expect(screen.getByTestId('form-problem-name')).toBeTruthy();
    await fireEvent.input(screen.getByTestId('form-field-name'), { target: { value: 'pos' } });
    await fireEvent.keyDown(screen.getByTestId('form-field-name'), { key: 'Enter' });
    expect(screen.getByTestId('form-step-title').textContent).toBe('More');
    // Two fields: Enter in the text input does nothing; Ctrl+Enter submits.
    await fireEvent.keyDown(screen.getByTestId('form-field-repo'), { key: 'Enter' });
    expect(run).not.toHaveBeenCalled();
    await fireEvent.keyDown(screen.getByTestId('form-field-note'), { key: 'Enter', ctrlKey: true });
    expect(run).toHaveBeenCalledWith({ name: 'pos' });
  });

  it('puts the failure on top, and closing a filled wizard asks once', async () => {
    const onclose = vi.fn();
    render(WizardDialog, { wizard: required, error: 'Only the owner adds projects.', run: () => {}, onclose });
    const banner = screen.getByTestId('wizard-error');
    expect(banner.compareDocumentPosition(screen.getByTestId('form-field-name')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await fireEvent.input(screen.getByTestId('form-field-name'), { target: { value: 'pos' } });
    await fireEvent.click(screen.getByTestId('form-cancel'));
    expect(onclose).not.toHaveBeenCalled();
    expect(screen.getByTestId('form-discard-ask')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('form-cancel'));
    expect(onclose).toHaveBeenCalledOnce();
  });

  it('an untouched wizard closes at once', async () => {
    const onclose = vi.fn();
    render(WizardDialog, { wizard: required, run: () => {}, onclose });
    await fireEvent.click(screen.getByTestId('form-cancel'));
    expect(onclose).toHaveBeenCalledOnce();
  });
});
