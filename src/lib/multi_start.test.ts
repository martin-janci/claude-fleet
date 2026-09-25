// Multi-repo start (work graph M9.6): which projects the dialog offers, and
// the note on what a start left out.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('./result', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./result')>();
  return { ...actual, invokeCmd: vi.fn() };
});
import { invokeCmd } from './result';
import {
  siblingCandidates,
  multiStartNote,
  multiStartKind,
  multiStartToast,
  crossOrgRetry,
  shownSiblings,
  type MultiStart,
} from './multi_start';
import { toasts, clearToasts, runToastAction } from './toasts';
import type { ProjectTreeRow } from './projects';
import type { WorkLink } from './work';

const proj = (id: number, repo: string, system = false): ProjectTreeRow => ({
  project: { id, owner: 'acme', repo, base_path: `/p/${repo}`, last_session_at: null, system } as ProjectTreeRow['project'],
  worktrees: [],
});
const link = (pid: number | null, ended: number): WorkLink => ({
  id: ended,
  state: 'confirmed',
  source: 'manual',
  created_at: 1,
  ended_at: ended,
  snap_project_id: pid,
});

describe('siblingCandidates', () => {
  it('offers the projects the key ran in, newest first, never the chosen one', () => {
    const projects = [proj(1, 'app'), proj(2, 'web'), proj(3, 'api'), proj(4, 'ops', true)];
    const got = siblingCandidates(
      'ABC-7',
      1,
      [link(2, 10), link(1, 50), link(3, 30), link(4, 60), link(9, 70), link(null, 80)],
      [
        { project_id: 2, work: { link_id: 1, item_id: null, key: 'ABC-7', title: '', source: 'manual' }, last_activity_at: 40 },
        { project_id: 3, work: { link_id: 2, item_id: null, key: 'OTHER-1', title: '', source: 'manual' }, last_activity_at: 99 },
      ],
      projects,
    );
    expect(got).toEqual([
      { id: 2, label: 'acme/web' },
      { id: 3, label: 'acme/api' },
    ]);
  });
});

describe('multiStartNote', () => {
  it('names what was skipped or failed, and says nothing otherwise', () => {
    const label = (id: number) => `p${id}`;
    expect(multiStartNote({ key: 'A-1', started: [] }, label)).toBeNull();
    expect(
      multiStartNote(
        {
          key: 'A-1',
          started: [],
          skipped: [{ project_id: 1, session_id: 5, reason: 'x' }],
          failed: [{ project_id: 2, code: 'E_SSH', message: 'host down' }],
        },
        label,
      ),
    ).toBe('Started 0; p1: already running; p2: host down');
  });
});

describe('shownSiblings', () => {
  it('keeps only ticked projects the dialog still offers, in ticking order', () => {
    const offered = [
      { id: 2, label: 'acme/web' },
      { id: 3, label: 'acme/api' },
    ];
    expect(shownSiblings([3, 9, 2], offered)).toEqual([3, 2]);
    expect(shownSiblings([9], offered)).toEqual([]);
    expect(shownSiblings([2], [])).toEqual([]);
    expect(shownSiblings([], offered)).toEqual([]);
  });
});

describe('the multi-start toast (M10.1)', () => {
  const invoked = invokeCmd as ReturnType<typeof vi.fn>;
  const label = (id: number) => `p${id}`;
  beforeEach(() => {
    invoked.mockReset();
    clearToasts();
  });

  it('is a warning when anything failed, half-started or ran out of time; info otherwise', () => {
    const only: MultiStart = { key: 'A-1', started: [], skipped: [{ project_id: 1, reason: 'x' }] };
    expect(multiStartKind(only)).toBe('info');
    expect(multiStartKind({ key: 'A-1', started: [], failed: [{ project_id: 2, code: 'E_SSH', message: 'down' }] })).toBe('warning');
    expect(
      multiStartKind({ key: 'A-1', started: [], warnings: [{ project_id: 2, session_id: 9, code: 'E_SQLITE', message: 'x' }] }),
    ).toBe('warning');
    const late: MultiStart = { key: 'A-1', started: [], skipped: [{ project_id: 3, reason: 'deadline' }] };
    expect(multiStartKind(late)).toBe('warning');
    expect(multiStartNote(late, label)).toBe('Started 0; p3: not started, out of time');
    expect(
      multiStartNote({ key: 'A-1', started: [], warnings: [{ project_id: 2, session_id: 9, code: 'E_SQLITE', message: 'its link failed' }] }, label),
    ).toBe('Started 0; p2: started, but its link failed');
  });

  it('offers "Start anyway" for a cross-org failure, re-calling with force_cross_org in those repos only', async () => {
    const r: MultiStart = {
      key: 'B-2',
      started: [],
      failed: [
        { project_id: 1, code: 'E_FORBIDDEN', message: 'B-2 belongs to organisation 2', cross_org: true },
        { project_id: 2, code: 'E_SSH', message: 'host down' },
      ],
    };
    expect(crossOrgRetry(r)).toEqual([1]);
    const args = { reference: 'B-2', project_ids: [1, 2], host_alias: 'h' };
    const t = multiStartToast(args, r, label)!;
    expect(t.kind).toBe('warning');
    expect(t.action?.label).toBe('Start anyway');
    invoked.mockResolvedValueOnce({ ok: true, value: { key: 'B-2', started: [] } });
    const { push } = await import('./toasts');
    push(t);
    const id = get(toasts)[0].id;
    runToastAction(id);
    await vi.waitFor(() => expect(invoked).toHaveBeenCalled());
    expect(invoked).toHaveBeenCalledWith('start_work_multi', {
      args: { reference: 'B-2', project_ids: [1], host_alias: 'h', force_cross_org: true },
    });
    // A forced start never offers itself again.
    expect(multiStartToast({ ...args, force_cross_org: true }, r, label)?.action).toBeUndefined();
    // No cross-org failure: no action.
    expect(multiStartToast(args, { key: 'B-2', started: [], failed: [r.failed![1]] }, label)?.action).toBeUndefined();
  });
});
