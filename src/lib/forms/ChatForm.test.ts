import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import ChatForm, { type ChatFormOutcome } from './ChatForm.svelte';
import { expectAccessible } from '../a11y_check';
import type { FormSpec } from './forms';
import { loadSaved } from './saved_answers';

const SPEC: FormSpec = {
  spec: 'fleet.form/1',
  title: 'New project',
  submit: 'Create',
  steps: [
    {
      title: 'Basics',
      fields: [
        { name: 'name', type: 'text', label: 'Name', required: true },
        { name: 'host', type: 'select', label: 'Host', required: true, options: [['mercury', 'mercury'], ['mac', 'mac']] },
      ],
    },
    { title: 'Where', fields: [{ name: 'draft_pr', type: 'bool', label: 'Open a draft PR' }] },
  ],
};
const TEXT = JSON.stringify(SPEC);

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

async function fill() {
  await fireEvent.input(screen.getByTestId('form-field-name'), { target: { value: 'papaya-receipts-v2' } });
  await fireEvent.click(screen.getByTestId('form-field-host-mercury'));
  await fireEvent.click(screen.getByTestId('form-next'));
  await fireEvent.click(screen.getByTestId('form-field-draft_pr'));
}

describe('ChatForm: a wizard in the chat (redesign step 10.12)', () => {
  it('draws the spec in while it is written, then opens it once whole', async () => {
    const r = render(ChatForm, {
      props: { draft: TEXT.slice(0, TEXT.indexOf('"label":"Host"')), from: 'Control', reading: '2 repos', onsubmit: vi.fn() },
    });
    expect(screen.getByTestId('chat-form-building').textContent).toContain('reading 2 repos');
    expect(screen.getByTestId('chat-form-draft-title').textContent).toBe('New project');
    expect(screen.getByTestId('chat-form-draft-name')).toBeInTheDocument();
    expect(screen.queryByTestId('chat-form-draft-host')).toBeNull();
    // Nothing to press while it is being written.
    expect(screen.queryByTestId('form-next')).toBeNull();

    await r.rerender({ draft: TEXT, from: 'Control', onsubmit: vi.fn() });
    expect(screen.queryByTestId('chat-form-building')).toBeNull();
    expect(screen.getByTestId('form-step-count').textContent).toBe('Step 1 of 2');
    expect(screen.getByTestId('chat-form').textContent).toContain('Nothing runs until you press Create');
  });

  it('runs only on the last button, with a Comet in it, then shrinks to one line and a Pulse', async () => {
    const done = deferred<ChatFormOutcome>();
    const onsubmit = vi.fn(() => done.promise);
    render(ChatForm, { props: { spec: SPEC, from: 'Control', sending: 'Creating…', onsubmit } });
    await fill();
    expect(onsubmit).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByTestId('form-submit'));
    expect(onsubmit).toHaveBeenCalledWith({ name: 'papaya-receipts-v2', host: 'mercury', draft_pr: true });
    const button = screen.getByTestId('form-submit');
    expect(button.textContent).toContain('Creating…');
    expect(button.querySelector('[data-testid^="loader"]')).not.toBeNull();

    done.resolve({ ok: true, starting: 'Starting papaya-receipts-v2' });
    const outcome = await screen.findByTestId('chat-form-outcome');
    expect(outcome.dataset.state).toBe('answered');
    expect(outcome.textContent).toContain('answered by you');
    expect(screen.getByTestId('chat-form-summary').textContent).toBe('papaya-receipts-v2 · mercury · open a draft PR on');
    expect(screen.getByTestId('chat-form-starting').textContent).toContain('Starting papaya-receipts-v2');
    expect(screen.queryByTestId('chat-form')).toBeNull();
  });

  it('keeps the person on the step with the problem when the run refuses', async () => {
    render(ChatForm, {
      props: {
        spec: SPEC,
        from: 'Control',
        onsubmit: async () => ({ ok: false, problems: [{ field: 'name', problem: 'is taken' }] }),
      },
    });
    await fill();
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect((await screen.findByTestId('form-problem-name')).textContent).toBe('is taken');
    expect(screen.getByTestId('form-step-title').textContent).toBe('Basics');
    expect(screen.queryByTestId('chat-form-outcome')).toBeNull();
  });

  it('declines with a note and runs nothing', async () => {
    const onsubmit = vi.fn();
    const ondecline = vi.fn();
    render(ChatForm, { props: { spec: SPEC, from: 'Control', onsubmit, ondecline } });
    await fireEvent.click(screen.getByTestId('chat-form-decline'));
    await fireEvent.input(screen.getByTestId('chat-form-decline-note'), { target: { value: 'Not today' } });
    await fireEvent.click(screen.getByTestId('chat-form-decline-confirm'));
    await waitFor(() => expect(screen.getByTestId('chat-form-outcome').dataset.state).toBe('declined'));
    expect(ondecline).toHaveBeenCalledWith('Not today');
    expect(onsubmit).not.toHaveBeenCalled();
    expect(screen.getByTestId('chat-form-outcome').textContent).toContain('“Not today”');
  });

  it('is accessible', async () => {
    const { container } = render(ChatForm, { props: { spec: SPEC, from: 'Control', onsubmit: vi.fn() } });
    await expectAccessible(container);
    await fill();
    await expectAccessible(container);
  });
});

// G1.3: the newer fleet.form/1 keys drawn in the chat, through the same
// FormWizard a dialog and an agent's FormCard use.
const WIDE: FormSpec = {
  spec: 'fleet.form/1',
  title: 'Deploy',
  submit: 'Deploy',
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
          required: true,
          other: true,
          options: [
            { value: 'mercury', label: 'mercury', detail: 'next free on main' },
            { value: 'venus', label: 'venus', detail: '2 idle', proposed: { by: 'rule', reason: 'it has the most room' } },
          ],
        },
      ],
    },
    {
      title: 'What ships',
      name: 'What',
      fields: [
        { name: 'summary', type: 'text', label: 'Summary', value: 'Ship the hub fix', drafted: { by: 'haiku on mercury', from: 'the Jira epic' } },
        { name: 'tier', type: 'select', label: 'Tier', value: 'small', disabled_reason: 'Larger tiers need an org admin', options: [['small', 'Small'], ['large', 'Large']] },
        { name: 'token', type: 'secret', label: 'Token', secret_note: 'Stays on mercury' },
      ],
    },
    { title: 'Check it', name: 'Review', kind: 'review' },
  ] as FormSpec['steps'],
};

describe('ChatForm: the wider spec (G1.3)', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('draws step chips by name, a proposal with its reason, each option’s detail', () => {
    render(ChatForm, { props: { spec: WIDE, from: 'Control', onsubmit: vi.fn() } });
    const chips = screen.getByTestId('form-step-chips');
    expect(chips).toHaveTextContent('Where');
    expect(chips).toHaveTextContent('What');
    expect(chips).toHaveTextContent('Review');
    expect(screen.getAllByRole('radio')[0]).toHaveAttribute('data-testid', 'form-field-host-venus');
    expect(screen.getByTestId('form-field-host-venus')).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByTestId('form-proposed-host')).toHaveTextContent('it has the most room');
    expect(screen.getByTestId('form-option-detail-mercury')).toHaveTextContent('next free on main');
    expect(screen.getByTestId('form-option-detail-venus')).toHaveTextContent('2 idle');
  });

  it('takes Another…, shows the drafted, disabled and secret lines, reviews with Edit, and sends only what is answered', async () => {
    const onsubmit = vi.fn(async () => ({ ok: true as const }));
    render(ChatForm, { props: { spec: WIDE, from: 'Control', onsubmit } });
    await fireEvent.click(screen.getByTestId('form-field-host-other'));
    await fireEvent.input(screen.getByTestId('form-field-host-other-text'), { target: { value: 'pluto' } });
    await fireEvent.click(screen.getByTestId('form-next'));

    expect(screen.getByTestId('form-drafted-from-summary')).toHaveTextContent('by haiku on mercury · from the Jira epic');
    expect(screen.getByTestId('form-disabled-tier')).toHaveTextContent('Larger tiers need an org admin');
    expect(screen.getByTestId('form-secret-note-token')).toHaveTextContent('Stays on mercury');
    await fireEvent.input(screen.getByTestId('form-field-token'), { target: { value: 's3cret' } });
    await fireEvent.click(screen.getByTestId('form-next'));

    const review = screen.getByTestId('form-review');
    expect(screen.getByTestId('form-review-step-0')).toHaveTextContent('pluto');
    expect(review).toHaveTextContent('set, never shown to the agent');
    expect(review).not.toHaveTextContent('s3cret');
    await fireEvent.click(screen.getByTestId('form-review-edit-0'));
    expect(screen.getByTestId('form-step-title')).toHaveTextContent('Where it runs');
    expect(screen.getByTestId('form-field-host-other-text')).toHaveValue('pluto');
    await fireEvent.click(screen.getByTestId('form-step-chip-0'));
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.click(screen.getByTestId('form-submit'));

    // The disabled tier is shown, never sent.
    expect(onsubmit).toHaveBeenCalledWith({ host: 'pluto', summary: 'Ship the hub fix', token: 's3cret' });
    expect((await screen.findByTestId('chat-form-summary')).textContent).toBe('pluto · Ship the hub fix · 1 secret');
  });

  it('saves to finish later: one line with Resume, the answers kept on the device without the secret, gone once answered', async () => {
    const onsubmit = vi.fn(async () => ({ ok: true as const }));
    const first = render(ChatForm, { props: { spec: WIDE, from: 'Control', saveKey: 'wizard:app:1:k', onsubmit } });
    await fireEvent.click(screen.getByTestId('form-field-host-mercury'));
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.input(screen.getByTestId('form-field-token'), { target: { value: 's3cret' } });
    await fireEvent.click(screen.getByTestId('form-save-later'));

    expect(screen.queryByTestId('chat-form')).toBeNull();
    expect(screen.getByTestId('form-saved-later')).toHaveTextContent('Deploy');
    expect(loadSaved('wizard:app:1:k')).toEqual({ host: 'mercury', summary: 'Ship the hub fix' });
    await fireEvent.click(screen.getByTestId('form-resume'));
    expect(screen.getByTestId('chat-form')).toBeInTheDocument();
    first.unmount();

    // Drawn again (the panel re-mounted): it opens from what was kept.
    render(ChatForm, { props: { spec: WIDE, from: 'Control', saveKey: 'wizard:app:1:k', onsubmit } });
    expect(screen.getByTestId('form-field-host-mercury')).toHaveAttribute('aria-checked', 'true');
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.input(screen.getByTestId('form-field-token'), { target: { value: 's3cret' } });
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.click(screen.getByTestId('form-submit'));
    await screen.findByTestId('chat-form-outcome');
    expect(loadSaved('wizard:app:1:k')).toBeNull();
  });

  it('offers no Save and finish later without a place to keep the answers', () => {
    render(ChatForm, { props: { spec: WIDE, from: 'Control', onsubmit: vi.fn() } });
    expect(screen.queryByTestId('form-save-later')).toBeNull();
  });
});
