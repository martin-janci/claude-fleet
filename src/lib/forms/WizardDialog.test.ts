import { render, screen } from '@testing-library/svelte';
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
