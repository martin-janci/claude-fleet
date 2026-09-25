// Multi-repo start (work graph M9.6): which projects the dialog offers, and
// the note on what a start left out.
import { describe, it, expect } from 'vitest';
import { siblingCandidates, multiStartNote, shownSiblings } from './multi_start';
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
