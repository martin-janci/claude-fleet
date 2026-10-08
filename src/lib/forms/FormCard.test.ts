import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FormCard from './FormCard.svelte';
import type { FormView } from './forms';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

function view(over: Partial<FormView> = {}): FormView {
  return {
    form_id: 'f_a',
    session_id: 4,
    host_alias: 'h',
    title: 'Deploy',
    why: 'to pick a target',
    state: 'pending',
    created_at: 1,
    spec: {
      spec: 'fleet.form/1',
      title: 'Deploy',
      submit: 'Go',
      steps: [
        { title: 'Target', fields: [
          { name: 'env', type: 'select', label: 'Env', required: true, options: [['stg', 'Staging'], ['prod', 'Production']] },
          { name: 'extra', type: 'bool', label: 'More options' } ] },
        { title: 'More', when: { field: 'extra', truthy: true }, fields: [
          { name: 'pw', type: 'secret', label: 'Password', required: true } ] },
      ],
    },
    ...over,
  };
}

// braces: a bare `() => inv.mockReset()` returns the mock, which vitest then
// calls as the teardown, and the mock's throwing default fails the test.
beforeEach(() => {
  inv.mockReset();
});

describe('FormCard', () => {
  it('walks the steps, keeps values on Back, and submits typed answers', async () => {
    inv.mockImplementation(async (cmd: string) => (cmd === 'get_form' ? view() : view({ state: 'answered' })));
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    expect(await screen.findByTestId('form-why')).toHaveTextContent('to pick a target');
    expect(screen.queryByTestId('form-step-count')).toBeNull();

    const next = () => screen.getByTestId('form-next');
    expect(screen.getByTestId('form-submit')).toBeDisabled();
    await fireEvent.change(screen.getByTestId('form-field-env'), { target: { value: 'prod' } });
    await fireEvent.click(screen.getByTestId('form-field-extra'));
    expect(await screen.findByTestId('form-step-count')).toHaveTextContent('Step 1 of 2');
    await fireEvent.click(next());
    expect(screen.getByTestId('form-step-title')).toHaveTextContent('More');
    await fireEvent.click(screen.getByTestId('form-back'));
    expect((screen.getByTestId('form-field-env') as HTMLSelectElement).value).toBe('prod');
    await fireEvent.click(next());
    await fireEvent.input(screen.getByTestId('form-field-pw'), { target: { value: 'hunter2' } });
    await fireEvent.click(screen.getByTestId('form-submit'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('answer_form', { formId: 'f_a', values: { env: 'prod', extra: true, pw: 'hunter2' } }),
    );
  });

  it('shows the server’s per-field problems, stays open, and clears a sent secret', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_form') return view();
      throw {
        code: 'E_INVALID',
        message: 'env: must be one of the options',
        details: { problems: [{ field: 'env', problem: 'must be one of the options' }, { field: 'pw', problem: 'is too weak' }] },
      };
    });
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    await fireEvent.change(await screen.findByTestId('form-field-env'), { target: { value: 'stg' } });
    await fireEvent.click(screen.getByTestId('form-field-extra'));
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.input(screen.getByTestId('form-field-pw'), { target: { value: 'hunter2' } });
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect(await screen.findByTestId('form-problem-pw')).toHaveTextContent('is too weak');
    // still mounted, and the secret that was sent is gone from the field
    expect((screen.getByTestId('form-field-pw') as HTMLInputElement).value).toBe('');
    await fireEvent.click(screen.getByTestId('form-back'));
    expect(await screen.findByTestId('form-problem-env')).toHaveTextContent('must be one of the options');
  });

  it('declines with a reason', async () => {
    inv.mockImplementation(async (cmd: string) => (cmd === 'get_form' ? view() : view({ state: 'declined' })));
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    await fireEvent.click(await screen.findByTestId('form-decline'));
    await fireEvent.input(screen.getByTestId('form-decline-note'), { target: { value: 'not today' } });
    await fireEvent.click(screen.getByTestId('form-decline-confirm'));
    await waitFor(() => expect(inv).toHaveBeenCalledWith('decline_form', { formId: 'f_a', note: 'not today' }));
  });

  it('is read-only with the reason when this client may not answer', async () => {
    inv.mockImplementation(async () => view());
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: 'You can watch this session, not drive it.' } });
    expect(await screen.findByTestId('form-blocked')).toHaveTextContent('not drive it');
    expect(screen.getByTestId('form-field-env')).toBeDisabled();
    expect(screen.queryByTestId('form-decline')).toBeNull();
  });

  it('says how a closed form ended', async () => {
    inv.mockImplementation(async () => view({ state: 'answered', answered_by: 'phone (device)' }));
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null, closed: true } });
    expect(await screen.findByTestId('form-outcome')).toHaveTextContent('Deploy: answered by phone (device)');
  });
});
