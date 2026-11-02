// The AI check card (docs/work-graph.md#ai-check-on-done): the verdict, Test
// now / Rerun, Accept anyway and the project's switch.
import { render, fireEvent } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as realInvoke } from '@tauri-apps/api/core';
import TicketCheckCard from './TicketCheckCard.svelte';
import { canAccept, checkHeadline, runLabel, type TicketCheckView } from './ticket_check';
import { STATUS_WORDS } from './kit/status';

const invoke = realInvoke as unknown as ReturnType<typeof vi.fn>;

const setup = { project_id: 7, project: 'acme/api', mode: 'ask' as const, command: 'cargo test' };
const failed: TicketCheckView = {
  state: 'failed',
  setup,
  task_id: 40,
  at: 1,
  summary: 'The login test fails.',
  blockers: ['login() returns 500 on an empty password'],
  tests_run: ['cargo test'],
  can_run: true,
};

beforeEach(() => invoke.mockReset());

describe('ticket_check', () => {
  it('names every state with a status word', () => {
    for (const state of ['ready', 'running', 'passed', 'failed', 'error', 'accepted'] as const) {
      const word = checkHeadline({ state }).label.split(' · ')[0];
      expect(STATUS_WORDS).toContain(word);
    }
  });
  it('offers Test now before the first check and Accept anyway only after a failed one', () => {
    expect(runLabel({ state: 'ready' })).toBe('Test now');
    expect(runLabel(failed)).toBe('Rerun');
    expect(canAccept(failed)).toBe(true);
    expect(canAccept({ state: 'error' })).toBe(true);
    expect(canAccept({ state: 'passed' })).toBe(false);
  });
});

describe('TicketCheckCard', () => {
  it('shows what must change and reruns the check', async () => {
    invoke.mockResolvedValue({ id: 41 });
    const onChanged = vi.fn();
    const r = render(TicketCheckCard, { check: failed, itemId: 2, onChanged });
    expect(r.getByTestId('ticket-check-headline').textContent).toContain('Failed');
    expect(r.getByTestId('ticket-check-blockers').textContent).toContain('empty password');
    await fireEvent.click(r.getByTestId('ticket-check-run'));
    await vi.waitFor(() => expect(onChanged).toHaveBeenCalled());
    expect(invoke).toHaveBeenCalledWith('run_ticket_check', { args: { item_id: 2 } });
  });

  it('accepts a failed check anyway with the person’s note', async () => {
    invoke.mockResolvedValue({ item_id: 2, line: 'ai check', ok: true });
    const r = render(TicketCheckCard, { check: failed, itemId: 2 });
    await fireEvent.click(r.getByTestId('ticket-check-accept'));
    const note = r.getByLabelText('Why it is fine');
    await fireEvent.input(note, { target: { value: 'flaky on CI only' } });
    await fireEvent.click(r.getByText('Accept'));
    await vi.waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('verify_work_item', {
        args: { item_id: 2, line: 'ai check', ok: true, note: 'flaky on CI only' },
      }),
    );
  });

  it('turns the project’s check on from the card', async () => {
    invoke.mockResolvedValue({ ...setup, mode: 'auto' });
    const r = render(TicketCheckCard, { check: { state: 'ready', setup, can_run: true }, itemId: 2 });
    expect(r.getByTestId('ticket-check-run').textContent).toContain('Test now');
    await fireEvent.click(r.getByTestId('ticket-check-setup'));
    await fireEvent.click(r.getByLabelText('When done'));
    await fireEvent.click(r.getByTestId('ticket-check-save'));
    await vi.waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_ticket_check', {
        args: { project_id: 7, mode: 'auto', command: 'cargo test' },
      }),
    );
  });

  it('says why a check cannot start instead of offering one', () => {
    const r = render(TicketCheckCard, {
      check: { state: 'passed', can_run: false, why_not: 'a check is already running' },
      itemId: 2,
    });
    expect(r.queryByTestId('ticket-check-run')).toBeNull();
    expect(r.getByTestId('ticket-check-why-not').textContent).toContain('already running');
    expect(r.queryByTestId('ticket-check-accept')).toBeNull();
  });
});
