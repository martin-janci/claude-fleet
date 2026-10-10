// The newer fleet.form/1 keys as FormWizard draws them: step chips, a
// review step with Edit links, Save and finish later, an option's detail
// and proposal, Another…, a disabled field's reason, a drafted default and
// a secret's note.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import FormWizard from './FormWizard.svelte';
import type { FormSpec } from './forms';
import { loadSaved } from './saved_answers';

const spec: FormSpec = {
  spec: 'fleet.form/1',
  title: 'Deploy',
  save_later: true,
  steps: [
    {
      title: 'Where it runs',
      name: 'Where',
      fields: [
        {
          name: 'host',
          type: 'select',
          label: 'Host',
          other: true,
          options: [
            ['mercury', 'mercury'],
            { value: 'venus', label: 'venus', detail: '2 idle', proposed: { by: 'jev', reason: 'the last three ran there' } },
          ],
        },
        { name: 'tier', type: 'select', label: 'Tier', value: 'small', disabled_reason: 'Larger tiers need an org admin', options: [['small', 'Small'], ['large', 'Large']] },
        { name: 'summary', type: 'text', label: 'Summary', value: 'Ship the hub fix', drafted: { by: 'haiku on mercury', from: 'the Jira epic' } },
      ],
    },
    { title: 'Secrets', fields: [{ name: 'token', type: 'secret', label: 'Token', secret_note: 'Written to a file on mercury, never shown to the agent' }] },
    { title: 'Check it', name: 'Review', kind: 'review' },
  ] as FormSpec['steps'],
};

beforeEach(() => {
  localStorage.clear();
});

describe('FormWizard, the newer keys', () => {
  it('draws the proposed option first with its reason, the detail line, and Change clears it', async () => {
    render(FormWizard, { props: { spec, onsubmit: vi.fn() } });
    const opts = screen.getAllByRole('radio');
    expect(opts[0]).toHaveAttribute('data-testid', 'form-field-host-venus');
    expect(opts[0]).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByTestId('form-option-detail-venus')).toHaveTextContent('2 idle');
    const why = screen.getByTestId('form-proposed-host');
    expect(why).toHaveTextContent('Proposed by Jev');
    expect(why).toHaveTextContent('the last three ran there');
    await fireEvent.click(screen.getByTestId('form-proposed-host-change'));
    expect(screen.queryByTestId('form-proposed-host')).toBeNull();
    expect(screen.getByTestId('form-field-host-venus')).toHaveAttribute('aria-checked', 'false');
  });

  it('takes free text through Another…', async () => {
    const onsubmit = vi.fn();
    render(FormWizard, { props: { spec: { ...spec, steps: [spec.steps[0]] }, onsubmit } });
    await fireEvent.click(screen.getByTestId('form-field-host-other'));
    await fireEvent.input(screen.getByTestId('form-field-host-other-text'), { target: { value: 'pluto' } });
    await fireEvent.click(screen.getByTestId('form-submit'));
    // The disabled field is shown, never sent.
    expect(onsubmit).toHaveBeenCalledWith({ host: 'pluto', summary: 'Ship the hub fix' });
  });

  it('says why a field is disabled, marks a drafted default until it changes, and notes where a secret goes', async () => {
    render(FormWizard, { props: { spec, onsubmit: vi.fn() } });
    expect(screen.getByTestId('form-disabled-tier')).toHaveTextContent('Larger tiers need an org admin');
    expect(screen.getByTestId('form-field-tier-small')).toBeDisabled();
    expect(screen.getByTestId('form-drafted-summary')).toHaveTextContent('Drafted');
    expect(screen.getByTestId('form-drafted-from-summary')).toHaveTextContent('by haiku on mercury · from the Jira epic');
    await fireEvent.input(screen.getByTestId('form-field-summary'), { target: { value: 'My own words' } });
    expect(screen.queryByTestId('form-drafted-summary')).toBeNull();
    await fireEvent.click(screen.getByTestId('form-next'));
    expect(screen.getByTestId('form-secret-note-token')).toHaveTextContent('never shown to the agent');
  });

  it('shows step chips by name, a review step with Edit links back', async () => {
    render(FormWizard, { props: { spec, onsubmit: vi.fn() } });
    expect(screen.getByTestId('form-step-chips')).toHaveTextContent('Where');
    expect(screen.getByTestId('form-step-chip-2')).toHaveTextContent('Review');
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.input(screen.getByTestId('form-field-token'), { target: { value: 's3cret' } });
    await fireEvent.click(screen.getByTestId('form-next'));
    const review = screen.getByTestId('form-review');
    expect(review).toHaveTextContent('venus');
    expect(review).toHaveTextContent('set, never shown to the agent');
    expect(review).not.toHaveTextContent('s3cret');
    expect(screen.getByTestId('form-step-chip-0')).toHaveTextContent('✓ Where');
    await fireEvent.click(screen.getByTestId('form-review-edit-0'));
    expect(screen.getByTestId('form-step-title')).toHaveTextContent('Where it runs');
  });

  it('keeps the answers, never a secret, on Save and finish later, and starts from them again', async () => {
    const onsavelater = vi.fn();
    const { unmount } = render(FormWizard, { props: { spec, saveKey: 'f_1', onsubmit: vi.fn(), onsavelater } });
    await fireEvent.input(screen.getByTestId('form-field-summary'), { target: { value: 'Half done' } });
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.input(screen.getByTestId('form-field-token'), { target: { value: 's3cret' } });
    await fireEvent.click(screen.getByTestId('form-save-later'));
    expect(onsavelater).toHaveBeenCalled();
    expect(loadSaved('f_1')).toEqual({ host: 'venus', summary: 'Half done' });
    unmount();
    render(FormWizard, { props: { spec, saveKey: 'f_1', onsubmit: vi.fn() } });
    expect(screen.getByTestId('form-field-summary')).toHaveValue('Half done');
  });

  it('offers no Save and finish later without the spec asking for it', () => {
    render(FormWizard, { props: { spec: { ...spec, save_later: false }, saveKey: 'f_1', onsubmit: vi.fn() } });
    expect(screen.queryByTestId('form-save-later')).toBeNull();
  });

  it('starts from kept answers only where the form still offers them', () => {
    localStorage.setItem('fleet.form.saved.f_2', JSON.stringify({ host: 'pluto', summary: 'Kept', gone: 'x' }));
    const closed = { ...spec, steps: [{ ...spec.steps[0], fields: spec.steps[0].fields.map((f) => (f.name === 'host' ? { ...f, other: false } : f)) }, ...spec.steps.slice(1)] };
    render(FormWizard, { props: { spec: closed, saveKey: 'f_2', onsubmit: vi.fn() } });
    // "pluto" is no host this form offers any more: the proposal stands.
    expect(screen.getByTestId('form-field-host-venus')).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByTestId('form-field-summary')).toHaveValue('Kept');
  });
});
