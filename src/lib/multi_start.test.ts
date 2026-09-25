// Multi-repo start (work graph M9.6): which projects the dialog offers, and
// the note on what a start left out.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { siblingCandidates, multiStartNote, shownSiblings, startWorkMulti } from './multi_start';
import { sessions, resetTombstonesForTests } from './sessions';
import { session } from './hosts_fixture';
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

describe('startWorkMulti', () => {
  beforeEach(() => {
    resetTombstonesForTests();
    sessions.set([]);
    vi.mocked(invoke).mockReset();
  });

  it('sends one start and takes every started row into the store', async () => {
    const other = session('h', 'other', { id: 70 });
    const app = session('h', 'app-abc-7', { id: 71, project_id: 1 });
    const web = session('h', 'web-abc-7', { id: 72, project_id: 2 });
    sessions.set([other]);
    vi.mocked(invoke).mockResolvedValue({
      key: 'ABC-7',
      started: [app, web],
      skipped: [{ project_id: 3, session_id: 5, reason: 'already running' }],
    });
    const args = { reference: 'ABC-7', project_id: 1, host_alias: 'h', name: 'abc-7', project_ids: [2, 3] };
    const r = await startWorkMulti(args);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('start_work_multi', { args });
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.value.started.map((s) => s.id)).toEqual([71, 72]);
      expect(r.value.skipped).toEqual([{ project_id: 3, session_id: 5, reason: 'already running' }]);
    }
    const ids = get(sessions).map((s) => s.id).sort((a, b) => a - b);
    expect(ids).toEqual([70, 71, 72]);
    // A started row that was already listed is replaced, not doubled.
    vi.mocked(invoke).mockResolvedValue({ key: 'ABC-7', started: [{ ...web, friendly_name: 'web' }] });
    await startWorkMulti(args);
    const rows = get(sessions);
    expect(rows).toHaveLength(3);
    expect(rows.find((s) => s.id === 72)?.friendly_name).toBe('web');
  });

  it('a refusal is the answer and the store is untouched', async () => {
    const other = session('h', 'other', { id: 70 });
    sessions.set([other]);
    vi.mocked(invoke).mockRejectedValue({ code: 'E_FORBIDDEN', message: 'ABC-7 is not visible' });
    const r = await startWorkMulti({ reference: 'ABC-7', project_id: 1, project_ids: [2] });
    expect(r).toEqual({ ok: false, error: { code: 'E_FORBIDDEN', message: 'ABC-7 is not visible' } });
    expect(get(sessions)).toEqual([other]);
  });

  it('an answer without a started list (an older hub, or nothing started) merges nothing and is still ok', async () => {
    sessions.set([]);
    vi.mocked(invoke).mockResolvedValue({ key: 'ABC-7', failed: [{ project_id: 2, code: 'E_SSH', message: 'host down' }] });
    const r = await startWorkMulti({ reference: 'ABC-7', project_id: 1, project_ids: [2] });
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value.failed?.[0].code).toBe('E_SSH');
    expect(get(sessions)).toEqual([]);
  });
});
