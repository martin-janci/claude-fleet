// Layout L3: the view only shows the step the backend sends and posts its
// values (the connect flow's logic is tested in crates/fleet-core/src/pages/
// flows.rs). A secret is never prefilled, and is cleared once sent.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FlowView from './FlowView.svelte';
import type { FlowStep } from './flows';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const urlStep: FlowStep = {
  flow_id: 'f1',
  flow: 'tracker.connect',
  step: 'url',
  title: 'Connect a tracker',
  intro: 'Paste any ticket or issue URL, or the site.',
  fields: [
    { name: 'url', label: 'Ticket or issue URL', type: 'text', placeholder: 'https://…', value: '', required: true },
    { name: 'picked', label: 'Tracker', type: 'select', options: [['', 'From the URL'], ['jira_dc', 'Jira Data Center']], value: '', required: false },
  ],
  submit: 'Next',
  back: false,
};

const detailsStep = (over: Partial<FlowStep> = {}): FlowStep => ({
  flow_id: 'f1',
  flow: 'tracker.connect',
  step: 'details',
  title: 'Connect Jira Cloud',
  intro: 'https://acme.atlassian.net · ABC-12',
  fields: [
    { name: 'email', label: 'Atlassian account email', type: 'text', placeholder: '', value: 'dev@acme.com', required: true },
    { name: 'token', label: 'API token', type: 'secret', value: '', required: true, help: 'Stored on this machine.' },
  ],
  submit: 'Connect',
  back: true,
  ...over,
});

beforeEach(() => inv.mockReset());

describe('FlowView', () => {
  it('shows the backend’s steps, posts their values, and says done', async () => {
    const outcomes = [
      { state: 'step', ...detailsStep() },
      { state: 'step', ...detailsStep({ error: 'Atlassian refused the token (401)' }) },
      { state: 'done', message: 'Connected https://acme.atlassian.net', record_id: 4 },
    ];
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'flow_start') return urlStep;
      if (cmd === 'flow_submit') return outcomes.shift();
      return null;
    });
    const ondone = vi.fn();
    render(FlowView, { props: { flow: 'tracker.connect', prefill: { url: 'x' }, ondone, oncancel: () => {} } });
    expect(await screen.findByTestId('flow-title')).toHaveTextContent('Connect a tracker');
    expect(inv).toHaveBeenCalledWith('flow_start', { flow: 'tracker.connect', prefill: { url: 'x' } });
    const next = screen.getByTestId('flow-submit') as HTMLButtonElement;
    expect(next.disabled).toBe(true);
    await fireEvent.input(screen.getByTestId('flow-field-url'), { target: { value: 'https://acme.atlassian.net/browse/ABC-12' } });
    await fireEvent.click(next);
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('flow_submit', {
        flowId: 'f1',
        values: { url: 'https://acme.atlassian.net/browse/ABC-12', picked: '' },
      }),
    );
    expect(await screen.findByTestId('flow-intro')).toHaveTextContent('ABC-12');
    const token = screen.getByTestId('flow-field-token') as HTMLInputElement;
    expect(token.type).toBe('password');
    expect(token.value).toBe('');
    await fireEvent.input(token, { target: { value: 'ATATT-secret' } });
    await fireEvent.click(screen.getByTestId('flow-submit'));
    // The same step, with the tracker's own error, and the token gone.
    expect(await screen.findByTestId('flow-error')).toHaveTextContent('refused the token');
    expect((screen.getByTestId('flow-field-token') as HTMLInputElement).value).toBe('');
    expect((screen.getByTestId('flow-field-email') as HTMLInputElement).value).toBe('dev@acme.com');
    await fireEvent.input(screen.getByTestId('flow-field-token'), { target: { value: 'ATATT-2' } });
    await fireEvent.click(screen.getByTestId('flow-submit'));
    await waitFor(() => expect(ondone).toHaveBeenCalledWith('Connected https://acme.atlassian.net', 4));
  });

  it('Back asks the backend for the first step; leaving an unfinished flow cancels it', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'flow_start') return detailsStep();
      if (cmd === 'flow_back') return { ...urlStep, fields: [{ ...urlStep.fields[0], value: 'https://kept' }, urlStep.fields[1]] };
      return null;
    });
    const oncancel = vi.fn();
    const { unmount } = render(FlowView, { props: { flow: 'tracker.connect', ondone: () => {}, oncancel } });
    await fireEvent.click(await screen.findByTestId('flow-back'));
    await waitFor(() => expect((screen.getByTestId('flow-field-url') as HTMLInputElement).value).toBe('https://kept'));
    await fireEvent.click(screen.getByTestId('flow-cancel'));
    expect(oncancel).toHaveBeenCalled();
    unmount();
    expect(inv).toHaveBeenCalledWith('flow_cancel', { flowId: 'f1' });
  });

  it('a refused start says why', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'flow_start') throw { code: 'E_LOCAL_ONLY', message: 'connect a tracker on the hub' };
      return null;
    });
    render(FlowView, { props: { flow: 'tracker.connect', ondone: () => {}, oncancel: () => {} } });
    expect(await screen.findByTestId('flow-error')).toHaveTextContent('on the hub');
  });
});
