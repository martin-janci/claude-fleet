import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(() => new Promise(() => {})) }));
import TransferChip from './TransferChip.svelte';
import { transferSheetFor, startMove, applyMoveProgress, resetMovesForTest } from './moves';
import { MOVE_STEPS, type MoveProgress, type MoveStep, type MoveStepState } from './moveProgress';
import { sessions, type SessionRow } from './sessions';

const movable = {
  id: 5, tmux_name: 'dev-foo', host_alias: 'alpha', kind: 'work', worktree_id: 10,
  project_id: 1, claude_session_id: 'x', parent_session_id: null, tags: [],
} as unknown as SessionRow;

const ev = (step: MoveStep, state: MoveStepState): MoveProgress => ({
  session_id: 5, to_host: 'beta', step, index: MOVE_STEPS.indexOf(step) + 1, total: 9, state, detail: null,
});

beforeEach(() => {
  resetMovesForTest();
  sessions.set([movable]);
});

describe('TransferChip', () => {
  it('idle and movable: a button on the host name that opens the sheet', async () => {
    const { container } = render(TransferChip, { props: { session: movable } });
    expect(container.textContent?.replace(/\s+/g, ' ').trim()).toContain('on alpha');
    await fireEvent.click(screen.getByTestId('transfer-chip'));
    expect(get(transferSheetFor)).toBe(5);
  });

  it('not movable: plain text, no button', () => {
    const { container } = render(TransferChip, { props: { session: { ...movable, kind: 'shell' } as SessionRow } });
    expect(container.textContent?.replace(/\s+/g, ' ').trim()).toBe('on alpha');
    expect(screen.queryByTestId('transfer-chip')).toBeNull();
  });

  it('running: the live indicator counts the step and reopens the sheet', async () => {
    startMove(movable, 'beta', { keepSource: false });
    applyMoveProgress(ev('replay', 'started'));
    render(TransferChip, { props: { session: movable } });
    await tick();
    const live = screen.getByTestId('transfer-live');
    expect(live.textContent?.replace(/\s+/g, ' ').trim()).toBe('⇄ moving to beta · 5/9');
    await fireEvent.click(live);
    expect(get(transferSheetFor)).toBe(5);
  });

  it('settled and not dismissed: says how it ended', async () => {
    applyMoveProgress(ev('check', 'started'));
    applyMoveProgress(ev('check', 'failed'));
    render(TransferChip, { props: { session: movable } });
    await tick();
    expect(screen.getByTestId('transfer-live').textContent).toContain('move failed');
  });
});
