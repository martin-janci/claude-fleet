// Attach a running session to a task (task → session spec J3): which
// sessions the picker offers and in what order, the Switch / Add default,
// the one write an attach makes and its Undo, and the picker itself.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { sessions, type SessionRow } from './sessions';
import { session } from './hosts_fixture';
import { attachCandidates, attachSession, defaultMode, type AttachTarget } from './attach';
import AttachPicker from './AttachPicker.svelte';

const onTask = (link_id: number, key: string) => ({ link_id, item_id: null, key, title: '', source: 'manual' });

const target: AttachTarget = {
  ref: { key: 'PAY-142' },
  key: 'PAY-142',
  linkedSessionIds: new Set([1]),
  projectIds: new Set([5]),
};

const already = session('oci', 'pay-api--142', { id: 1, project_id: 5, work: onTask(10, 'PAY-142') });
const otherRepoFree = session('oci', 'web--main', { id: 2, project_id: 6, claude_status: 'idle' });
const sameRepoBusy = session('oci', 'pay-api--fix-x', {
  id: 3,
  project_id: 5,
  claude_status: 'working',
  work: onTask(30, 'PAY-139'),
});
const sameRepoFree = session('oci', 'pay-api--main', { id: 4, project_id: 5, claude_status: 'idle' });

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;
function calls(cmd: string) {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  handlers = {};
  vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
    const h = handlers[cmd];
    return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
  });
});

describe('attachCandidates', () => {
  it('offers every session not on the task: same repo first, then no task, then idle', () => {
    const got = attachCandidates([already, otherRepoFree, sameRepoBusy, sameRepoFree], target).map((r) => r.id);
    expect(got).toEqual([4, 3, 2]);
  });

  it('never offers a lost session or the operator', () => {
    const lost = session('oci', 'gone', { id: 8, lost_at: 1 });
    const operator = session('oci', 'ux-agent', { id: 9 });
    const got = attachCandidates([lost, operator, otherRepoFree], { ...target, operatorId: 9 }).map((r) => r.id);
    expect(got).toEqual([2]);
  });

  it('filters by name, host or the key a session is on', () => {
    const rows = [otherRepoFree, sameRepoBusy, sameRepoFree];
    expect(attachCandidates(rows, target, 'web').map((r) => r.id)).toEqual([2]);
    expect(attachCandidates(rows, target, 'pay-139').map((r) => r.id)).toEqual([3]);
  });
});

describe('defaultMode', () => {
  it('switches an idle session, adds to one mid-turn or on a checkout named after its key', () => {
    expect(defaultMode({ ...sameRepoBusy, claude_status: 'idle' } as SessionRow)).toBe('switch');
    expect(defaultMode(sameRepoBusy)).toBe('add');
    expect(defaultMode({ ...sameRepoBusy, claude_status: 'idle', worktree_key: 'pay-139-refunds' } as SessionRow)).toBe('add');
  });
});

describe('attachSession', () => {
  it('a session with no task takes it as its primary, asking to be warned; Undo removes the link', async () => {
    handlers.link_session_work = () => ({ ...sameRepoFree, work: onTask(77, 'PAY-142') });
    handlers.unlink_session_work = () => sameRepoFree;
    const r = await attachSession(sameRepoFree, target, { mode: 'switch' });
    expect(r.ok).toBe(true);
    expect(calls('link_session_work')[0]).toEqual({ session_id: 4, key: 'PAY-142', primary: true, ack_live: false });
    if (!r.ok) return;
    await r.value.undo?.();
    expect(calls('unlink_session_work')[0]).toEqual({ session_id: 4, link_id: 77 });
  });

  it('a switch ends the other link as a compare-and-set; Undo switches back', async () => {
    handlers.switch_session_work = (a) =>
      a.link_id === 30 ? { ...sameRepoBusy, work: onTask(78, 'PAY-142') } : { ...sameRepoBusy, work: onTask(79, 'PAY-139') };
    const r = await attachSession(sameRepoBusy, target, { mode: 'switch', ackLive: true });
    expect(calls('switch_session_work')[0]).toEqual({
      session_id: 3,
      link_id: 30,
      key: 'PAY-142',
      expected_primary: 30,
      ack_live: true,
    });
    if (!r.ok) throw new Error('attach failed');
    await r.value.undo?.();
    expect(calls('switch_session_work')[1]).toEqual({
      session_id: 3,
      link_id: 78,
      key: 'PAY-139',
      expected_primary: 78,
      ack_live: true,
      force_cross_org: true,
    });
  });

  it('Undo of a switch across orgs goes back across orgs', async () => {
    handlers.switch_session_work = (a) =>
      a.link_id === 30 ? { ...sameRepoBusy, work: onTask(78, 'PAY-142') } : { ...sameRepoBusy, work: onTask(79, 'PAY-139') };
    const r = await attachSession(sameRepoBusy, target, { mode: 'switch', ackLive: true, forceCrossOrg: true });
    if (!r.ok) throw new Error('attach failed');
    await r.value.undo?.();
    expect(calls('switch_session_work')[1]).toMatchObject({ link_id: 78, force_cross_org: true });
  });

  it('Add links a secondary and leaves the primary; there is nothing to undo', async () => {
    handlers.link_session_work = () => sameRepoBusy;
    const r = await attachSession(sameRepoBusy, target, { mode: 'add' });
    expect(calls('link_session_work')[0]).toEqual({ session_id: 3, key: 'PAY-142', primary: false, ack_live: false });
    expect(r.ok && r.value.undo).toBe(null);
  });
});

describe('AttachPicker', () => {
  async function flush() {
    for (let i = 0; i < 10; i++) await tick();
  }

  it('defaults to Add mid-turn, says when the task is open elsewhere, then attaches anyway', async () => {
    sessions.set([already, sameRepoBusy, sameRepoFree]);
    let refused = false;
    handlers.link_session_work = () => {
      if (!refused) {
        refused = true;
        throw { code: 'E_EXISTS', message: 'live', details: { live_elsewhere: [{ message: 'Someone is already working on this.' }] } };
      }
      return sameRepoBusy;
    };
    const onattached = vi.fn();
    render(AttachPicker, { props: { target, heading: 'Attach a running session to PAY-142', onclose: vi.fn(), onattached } });
    await flush();
    const rows = screen.getAllByTestId('attach-picker-row');
    expect(rows.map((r) => r.getAttribute('data-session-id'))).toEqual(['4', '3']);
    await fireEvent.click(rows[1]);
    await flush();
    expect((screen.getByTestId('attach-picker-add') as HTMLInputElement).checked).toBe(true);
    await fireEvent.click(screen.getByTestId('attach-picker-go'));
    await flush();
    expect(screen.getByTestId('attach-picker-confirm').textContent).toContain('Someone is already working on this.');
    expect(screen.getByTestId('attach-picker-go').textContent).toBe('Attach anyway');
    await fireEvent.click(screen.getByTestId('attach-picker-go'));
    await flush();
    expect(calls('link_session_work')[1]).toEqual({ session_id: 3, key: 'PAY-142', primary: false, ack_live: true });
    expect(onattached).toHaveBeenCalledOnce();
    expect(onattached.mock.calls[0][1]).toBe('Added to pay-api--fix-x');
  });

  it('keeps both acknowledgements when a task is open elsewhere and in another org', async () => {
    sessions.set([already, sameRepoBusy, sameRepoFree]);
    // The backend asks P-3 first, then the org question, each until answered.
    handlers.link_session_work = (a) => {
      if (!a.ack_live) {
        throw { code: 'E_EXISTS', message: 'live', details: { live_elsewhere: [{ message: 'Open elsewhere.' }] } };
      }
      if (!a.force_cross_org) {
        throw { code: 'E_FORBIDDEN', message: 'org', details: { cross_org: true, work_org_id: 1, session_org_id: 2 } };
      }
      return sameRepoFree;
    };
    const onattached = vi.fn();
    render(AttachPicker, { props: { target, heading: 'Attach', onclose: vi.fn(), onattached } });
    await flush();
    await fireEvent.click(screen.getAllByTestId('attach-picker-row')[0]);
    await flush();
    for (let i = 0; i < 3; i++) {
      await fireEvent.click(screen.getByTestId('attach-picker-go'));
      await flush();
    }
    expect(calls('link_session_work')[2]).toEqual({
      session_id: 4,
      key: 'PAY-142',
      primary: true,
      ack_live: true,
      force_cross_org: true,
    });
    expect(onattached).toHaveBeenCalledOnce();
  });
});
