import { describe, expect, it } from 'vitest';
import { progressText, startSteps } from './start_progress';
import type { SessionEvent } from './timeline';

let next = 1;
function ev(kind: string, detail: string | null = null, at = next): SessionEvent {
  return { id: next++, session_id: 7, at, kind, detail, claude_session_id: null };
}
const spawned = (o: Partial<{ worktree_id: number | null; new_worktree: boolean; brief: boolean }> = {}) =>
  ev(
    'start_spawned',
    JSON.stringify({ key: 'FLEET-1', worktree_id: 3, new_worktree: true, branch: 'fleet-1', brief: true, ...o }),
  );
const states = (p: ReturnType<typeof startSteps>) => p?.steps.map((s) => `${s.id}:${s.state}`);

describe('startSteps', () => {
  it('is null for a session no start made', () => {
    expect(startSteps([ev('handover_started')])).toBeNull();
  });

  it('walks a start from spawned to brief sent, and offers Cancel until then', () => {
    const e = [spawned(), ev('worktree_ready', 'fleet-1')];
    let p = startSteps(e);
    expect(states(p)).toEqual(['spawned:done', 'worktree:done', 'repl:active', 'brief:pending']);
    expect(p?.cancellable).toBe(true);
    expect(progressText(p!)).toBe('Starting: claude ready…');

    e.push(ev('repl_ready'));
    p = startSteps(e);
    expect(states(p)).toEqual(['spawned:done', 'worktree:done', 'repl:done', 'brief:active']);

    e.push(ev('handover_started'));
    p = startSteps(e);
    expect(p?.done).toBe(true);
    expect(p?.cancellable).toBe(false);
    expect(progressText(p!)).toBe('Started: the agent has its brief');
  });

  it('reads events in any order (history comes newest first)', () => {
    const e = [spawned(), ev('repl_ready'), ev('handover_started')].reverse();
    expect(startSteps(e)?.done).toBe(true);
  });

  it('says when the person has to trust the folder, and clears it once the brief is in', () => {
    const e = [spawned(), ev('handover_waiting', 'trust_prompt')];
    let p = startSteps(e);
    expect(p?.waiting).toBe('trust_prompt');
    expect(progressText(p!)).toMatch(/trust this folder/);
    e.push(ev('repl_ready'), ev('handover_started'));
    p = startSteps(e);
    expect(p?.waiting).toBeNull();
    expect(p?.done).toBe(true);
  });

  it('names a brief that did not get through, and keeps Cancel', () => {
    const p = startSteps([spawned(), ev('handover_waiting', 'repl_not_ready')]);
    expect(p?.failed).toBe('Claude did not come up in time');
    expect(p?.steps.some((s) => s.state === 'active')).toBe(false);
    expect(p?.cancellable).toBe(true);
  });

  it('offers no Cancel in a checkout the start did not make', () => {
    expect(startSteps([spawned({ new_worktree: false })])?.cancellable).toBe(false);
    expect(startSteps([spawned({ worktree_id: null, new_worktree: false })])?.steps.map((s) => s.id)).toEqual([
      'spawned',
      'repl',
      'brief',
    ]);
  });

  it('is done at once for a start that sends no brief', () => {
    const p = startSteps([spawned({ brief: false })]);
    expect(p?.done).toBe(true);
    expect(states(p)).toEqual(['spawned:done', 'worktree:done']);
  });

  it('reads only the newest start of a reused session', () => {
    const p = startSteps([spawned(), ev('handover_started'), spawned()]);
    expect(p?.done).toBe(false);
  });

  it('shows a cancelled start as cancelled', () => {
    const p = startSteps([spawned(), ev('start_abandoned')]);
    expect(p?.abandoned).toBe(true);
    expect(p?.cancellable).toBe(false);
    expect(progressText(p!)).toBe('Start cancelled');
  });
});
