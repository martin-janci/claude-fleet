import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FormCard from './FormCard.svelte';
import type { FormView } from './forms';
import { loadSaved } from './saved_answers';

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
    await fireEvent.click(screen.getByTestId('form-field-env-prod'));
    await fireEvent.click(screen.getByTestId('form-field-extra'));
    expect(await screen.findByTestId('form-step-count')).toHaveTextContent('Step 1 of 2');
    await fireEvent.click(next());
    expect(screen.getByTestId('form-step-title')).toHaveTextContent('More');
    await fireEvent.click(screen.getByTestId('form-back'));
    expect(screen.getByTestId('form-field-env-prod')).toHaveAttribute('aria-checked', 'true');
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
    await fireEvent.click(await screen.findByTestId('form-field-env-stg'));
    await fireEvent.click(screen.getByTestId('form-field-extra'));
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.input(screen.getByTestId('form-field-pw'), { target: { value: 'hunter2' } });
    await fireEvent.click(screen.getByTestId('form-submit'));
    // the wizard goes to the first step with a problem
    expect(await screen.findByTestId('form-problem-env')).toHaveTextContent('must be one of the options');
    await fireEvent.click(screen.getByTestId('form-next'));
    // still mounted, the other problem shown, and the secret that was sent is gone
    expect(screen.getByTestId('form-problem-pw')).toHaveTextContent('is too weak');
    expect((screen.getByTestId('form-field-pw') as HTMLInputElement).value).toBe('');
  });

  it('moves to the step a server problem belongs to', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_form') return view();
      throw { code: 'E_INVALID', message: 'env: bad', details: { problems: [{ field: 'env', problem: 'must be one of the options' }] } };
    });
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    await fireEvent.click(await screen.findByTestId('form-field-env-stg'));
    await fireEvent.click(screen.getByTestId('form-field-extra'));
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.input(screen.getByTestId('form-field-pw'), { target: { value: 'hunter2' } });
    expect(screen.getByTestId('form-step-title')).toHaveTextContent('More');
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect(await screen.findByTestId('form-problem-env')).toHaveTextContent('must be one of the options');
    expect(screen.getByTestId('form-step-title')).toHaveTextContent('Target');
  });

  it('shows a problem for a field that is not on screen in form-error', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_form') return view();
      throw { code: 'E_INVALID', message: 'x', details: { problems: [{ field: 'ghost', problem: 'is not a field of this form' }] } };
    });
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    await fireEvent.click(await screen.findByTestId('form-field-env-stg'));
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect(await screen.findByTestId('form-error')).toHaveTextContent('ghost: is not a field of this form');
  });

  it('starts clean when it is pointed at another form', async () => {
    inv.mockImplementation(async (cmd: string, args?: { formId?: string }) => {
      if (cmd === 'get_form') return view({ form_id: args?.formId ?? 'f_a' });
      throw { code: 'E_INVALID', message: 'x', details: { problems: [{ field: 'env', problem: 'must be one of the options' }] } };
    });
    const { rerender } = render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    await fireEvent.click(await screen.findByTestId('form-field-env-stg'));
    await fireEvent.click(screen.getByTestId('form-submit'));
    await screen.findByTestId('form-problem-env');
    await fireEvent.click(screen.getByTestId('form-decline'));
    await rerender({ formId: 'f_b', sessionName: 'dev', blocked: null });
    await screen.findByTestId('form-field-env-stg');
    expect(screen.queryByTestId('form-problem-env')).toBeNull();
    expect(screen.queryByTestId('form-decline-note')).toBeNull();
  });

  it('gives each card its own input ids', async () => {
    inv.mockImplementation(async () => view());
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    render(FormCard, { props: { formId: 'f_b', sessionName: 'dev', blocked: null } });
    await waitFor(() => expect(screen.getAllByRole('radiogroup')).toHaveLength(2));
    const [a, b] = screen.getAllByRole('radiogroup').map((g) => g.getAttribute('aria-labelledby'));
    expect(a).not.toBe(b);
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
    expect(screen.getByTestId('form-field-env-stg')).toBeDisabled();
    expect(screen.queryByTestId('form-decline')).toBeNull();
  });

  it('says how a closed form ended', async () => {
    inv.mockImplementation(async () => view({ state: 'answered', answered_by: 'phone (device)' }));
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null, closed: true } });
    const r = await screen.findByTestId('form-outcome');
    expect(r).toHaveTextContent('Deploy');
    expect(screen.getByTestId('form-ended')).toHaveTextContent('answered on phone');
  });

  // ── Step 10.1: receipt, 1–9, expiry ──────────────────────────────────
  it('collapses to a receipt with the answer summary once answered, the answers behind Show answers', async () => {
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'get_form'
        ? view()
        : view({ state: 'answered', answered_by: 'Martin', decided_at: Math.floor(Date.now() / 1000), answers: { env: 'prod', extra: false } }),
    );
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    await fireEvent.click(await screen.findByTestId('form-field-env-prod'));
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect(await screen.findByTestId('form-ended')).toHaveTextContent(/^answered by Martin · \d\d:\d\d$/);
    expect(screen.queryByTestId('form-card')).toBeNull();
    expect(screen.getByTestId('form-summary')).toHaveTextContent('Production · more options off');
    expect(screen.queryByTestId('form-answers')).toBeNull();
    await fireEvent.click(screen.getByTestId('form-show-answers'));
    expect(screen.getByTestId('form-answers')).toHaveTextContent('EnvProduction');
  });

  it('a declined receipt keeps the note; an expired one says no one answered', async () => {
    inv.mockImplementation(async () => view({ state: 'declined', answered_by: 'Martin', note: 'Not today' }));
    const { unmount } = render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    expect(await screen.findByTestId('form-note')).toHaveTextContent('“Not today”');
    expect(screen.queryByTestId('form-show-answers')).toBeNull();
    unmount();
    inv.mockImplementation(async () => view({ state: 'expired' }));
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null, closed: true } });
    expect(await screen.findByTestId('form-outcome')).toHaveTextContent('No answer in 24 h.');
    expect(screen.getByTestId('form-ended')).toHaveTextContent('expired');
  });

  it('shows when a pending form expires', async () => {
    inv.mockImplementation(async () => view({ created_at: Math.floor(Date.now() / 1000) - (24 * 3600 - 9 * 60 - 30) }));
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    expect(await screen.findByTestId('form-expires')).toHaveTextContent('expires in 9 min');
  });

  it('numbers the step’s only choice and 1–9 pick from it', async () => {
    inv.mockImplementation(async () => view());
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    const prod = await screen.findByTestId('form-field-env-prod');
    expect(prod).toHaveTextContent('2');
    await fireEvent.keyDown(window, { key: '2' });
    expect(prod).toHaveAttribute('aria-checked', 'true');
    // a digit past the options, or typed into a field, picks nothing
    await fireEvent.keyDown(window, { key: '7' });
    expect(prod).toHaveAttribute('aria-checked', 'true');
    const input = document.createElement('input');
    document.body.append(input);
    await fireEvent.keyDown(input, { key: '1' });
    input.remove();
    expect(prod).toHaveAttribute('aria-checked', 'true');
  });

  it('takes no digits while blocked, with two choices on the step, or under a question card', async () => {
    const two = view();
    two.spec.steps[0].fields.push({ name: 'zone', type: 'select', label: 'Zone', options: [['a', 'A'], ['b', 'B']] });
    inv.mockImplementation(async () => two);
    const { unmount } = render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    const stg = await screen.findByTestId('form-field-env-stg');
    expect(stg).not.toHaveTextContent('1');
    await fireEvent.keyDown(window, { key: '1' });
    expect(stg).toHaveAttribute('aria-checked', 'false');
    unmount();

    inv.mockImplementation(async () => view());
    const card = document.createElement('div');
    card.dataset.testid = 'answer-card';
    document.body.append(card);
    const second = render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    await fireEvent.keyDown(window, { key: '1' });
    expect(await screen.findByTestId('form-field-env-stg')).toHaveAttribute('aria-checked', 'false');
    card.remove();
    second.unmount();

    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: 'You can watch this session, not drive it.' } });
    await screen.findByTestId('form-blocked');
    await fireEvent.keyDown(window, { key: '1' });
    expect(screen.getByTestId('form-field-env-stg')).toHaveAttribute('aria-checked', 'false');
  });

  it('picking the chosen option of an optional choice clears it', async () => {
    const opt = view();
    opt.spec.steps[0].fields[0].required = false;
    inv.mockImplementation(async () => opt);
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    const stg = await screen.findByTestId('form-field-env-stg');
    await fireEvent.click(stg);
    expect(stg).toHaveAttribute('aria-checked', 'true');
    await fireEvent.click(stg);
    expect(stg).toHaveAttribute('aria-checked', 'false');
  });

  // ── Step 10.9: Jev's quick answer on a form ─────────────────────────
  it('moves Jev’s likely option first and pre-selects it; Change puts it back', async () => {
    const v = view({ proposal: { field: 'env', value: 'qa', source: 'jev', confidence_pct: 70 } });
    v.spec.steps[0].fields[0].options = [['stg', 'Staging'], ['qa', 'QA']];
    inv.mockImplementation(async () => v);
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    const qa = await screen.findByTestId('form-field-env-qa');
    expect(qa).toHaveAttribute('aria-checked', 'true');
    expect(qa).toHaveClass('ai-pre');
    expect(screen.getAllByRole('radio').map((r) => r.textContent?.trim())).toEqual(['1 QA', '2 Staging']);
    expect(screen.getByTestId('form-proposed-env')).toHaveTextContent('Proposed by Jev');
    await fireEvent.click(screen.getByTestId('form-proposed-env-change'));
    expect(qa).toHaveAttribute('aria-checked', 'false');
    expect(qa).not.toHaveClass('ai-pre');
    expect(screen.getAllByRole('radio').map((r) => r.textContent?.trim())).toEqual(['1 Staging', '2 QA']);
  });

  // Review r15 F14: the field's meaning counts, not only its option words.
  it('never pre-selects a field that asks what AI never decides', async () => {
    const v = view({ proposal: { field: 'env', value: 'high', source: 'jev', confidence_pct: 95 } });
    v.spec.steps[0].fields[0].label = 'Priority';
    v.spec.steps[0].fields[0].options = [['low', 'Low'], ['high', 'High']];
    inv.mockImplementation(async () => v);
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    expect(await screen.findByTestId('form-field-env-high')).toHaveAttribute('aria-checked', 'false');
    expect(screen.queryByTestId('form-proposed-env')).toBeNull();
    expect(screen.getAllByRole('radio').map((r) => r.textContent?.trim())).toEqual(['1 Low', '2 High']);
  });

  it('never moves a risky option or a weak proposal', async () => {
    const risky = view({ proposal: { field: 'env', value: 'prod', source: 'jev', confidence_pct: 30 } });
    risky.spec.steps[0].fields[0].options = [['stg', 'Staging'], ['prod', 'Push to production']];
    inv.mockImplementation(async () => risky);
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    await screen.findByTestId('form-field-env-stg');
    expect(screen.queryByTestId('form-proposed-env')).toBeNull();
    expect(screen.getByTestId('form-field-env-prod')).toHaveAttribute('aria-checked', 'false');
  });

  // G1.3: an agent's form whose spec says `save_later` folds to one line
  // with Resume, keeps what was typed (never the secret) and forgets it once
  // declined.
  it('saves to finish later, resumes from the kept answers, and forgets them on Decline', async () => {
    localStorage.clear();
    const v = view();
    v.spec.save_later = true;
    inv.mockImplementation(async (cmd: string) => (cmd === 'get_form' ? v : view({ state: 'declined' })));
    const first = render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    await fireEvent.click(await screen.findByTestId('form-field-env-prod'));
    await fireEvent.click(screen.getByTestId('form-save-later'));
    expect(screen.getByTestId('form-saved-later')).toHaveTextContent('saved to finish later · expires');
    expect(screen.queryByTestId('form-card')).toBeNull();
    expect(loadSaved('f_a')).toEqual({ env: 'prod' });
    first.unmount();

    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    expect(await screen.findByTestId('form-field-env-prod')).toHaveAttribute('aria-checked', 'true');
    await fireEvent.click(screen.getByTestId('form-decline'));
    await fireEvent.click(screen.getByTestId('form-decline-confirm'));
    await screen.findByTestId('form-outcome');
    expect(loadSaved('f_a')).toBeNull();
  });
});
