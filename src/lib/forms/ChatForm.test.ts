import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import ChatForm, { type ChatFormOutcome } from './ChatForm.svelte';
import type { FormSpec } from './forms';

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
});
