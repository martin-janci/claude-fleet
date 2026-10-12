// Fork with a host choice (step 5.10): another host forks on this one, then
// moves the fork there; a failed move leaves the fork and says so.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return { ...actual, rewindConversation: vi.fn() };
});
vi.mock('./moveSession', async () => {
  const actual = await vi.importActual<typeof import('./moveSession')>('./moveSession');
  return { ...actual, moveSession: vi.fn() };
});
vi.mock('./hosts', async () => {
  const actual = await vi.importActual<typeof import('./hosts')>('./hosts');
  const { writable } = await import('svelte/store');
  const h = (alias: string, over = {}) => ({ alias, reachable: true, hidden: false, ...over });
  return {
    ...actual,
    hosts: writable([h('mac'), h('mercury'), h('nas', { reachable: false }), h('old', { hidden: true })]),
  };
});

import ForkSheet from './ForkSheet.svelte';
import { rewindConversation, sessions } from './sessions';
import { moveSession } from './moveSession';
import { session } from './hosts_fixture';

const rewind = rewindConversation as unknown as ReturnType<typeof vi.fn>;
const move = moveSession as unknown as ReturnType<typeof vi.fn>;

async function settle() {
  for (let i = 0; i < 6; i++) await tick();
}

beforeEach(() => {
  rewind.mockReset();
  move.mockReset();
  sessions.set([session('mac', 'canopus', { id: 1, friendly_name: 'Fix hub flake' })]);
});

const props = (onclose = vi.fn()) => ({
  props: { sessionId: 1, anchor: 'a-1', suggestedName: 'fork-of-canopus', onclose },
});

describe('ForkSheet host choice', () => {
  it('is titled after the session and offers only other reachable, visible hosts', async () => {
    render(ForkSheet, props());
    await settle();
    expect(screen.getByText('Fork "Fix hub flake"')).toBeTruthy();
    const opts = Array.from((screen.getByTestId('fork-host') as HTMLSelectElement).options).map((o) => o.value);
    expect(opts).toEqual(['', 'mercury']);
  });

  it('another host forces a new worktree, forks here, then moves the fork', async () => {
    rewind.mockResolvedValue({ ok: true, value: session('mac', 'fork', { id: 7 }) });
    move.mockResolvedValue({ ok: true, value: { kind: 'moved' } });
    const onclose = vi.fn();
    render(ForkSheet, props(onclose));
    await settle();
    await fireEvent.click(screen.getByTestId('fork-same-worktree'));
    await fireEvent.change(screen.getByTestId('fork-host'), { target: { value: 'mercury' } });
    await settle();
    expect((screen.getByTestId('fork-new-worktree') as HTMLInputElement).checked).toBe(true);
    expect((screen.getByTestId('fork-same-worktree') as HTMLInputElement).disabled).toBe(true);
    expect(screen.getByTestId('fork-move-note')).toHaveTextContent('mercury');
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    await settle();
    expect(rewind).toHaveBeenCalledWith(1, 'fork', 'a-1', 'fork-of-canopus');
    expect(move).toHaveBeenCalledWith(7, 'mercury', { when: 'idle' });
    expect(onclose).toHaveBeenCalled();
  });

  it('a move that waits for idle says so and keeps the sheet open until Done', async () => {
    rewind.mockResolvedValue({ ok: true, value: session('mac', 'fork', { id: 7 }) });
    move.mockResolvedValue({ ok: true, value: { kind: 'waiting' } });
    const onclose = vi.fn();
    render(ForkSheet, props(onclose));
    await settle();
    await fireEvent.change(screen.getByTestId('fork-host'), { target: { value: 'mercury' } });
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    await settle();
    expect(screen.getByTestId('fork-notice')).toHaveTextContent('moves to mercury as soon as it is idle');
    expect(onclose).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    expect(onclose).toHaveBeenCalled();
  });

  it('a failed move keeps the fork and says where it is', async () => {
    rewind.mockResolvedValue({ ok: true, value: session('mac', 'fork', { id: 7 }) });
    move.mockResolvedValue({ ok: false, error: { code: 'E_SSH', message: 'mercury is unreachable' } });
    render(ForkSheet, props());
    await settle();
    await fireEvent.change(screen.getByTestId('fork-host'), { target: { value: 'mercury' } });
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    await settle();
    expect(screen.getByTestId('fork-error')).toHaveTextContent('Forked on mac, but the move to mercury failed');
    expect(rewind).toHaveBeenCalledTimes(1);
  });

  it('this host forks without a move', async () => {
    rewind.mockResolvedValue({ ok: true, value: session('mac', 'fork', { id: 7 }) });
    render(ForkSheet, props());
    await settle();
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    await settle();
    expect(move).not.toHaveBeenCalled();
  });
});
