import { describe, it, expect } from 'vitest';
import {
  UNSURE_NOTE,
  confirmTitle,
  ignoredConversations,
  ignoredPanes,
  otherConversations,
  setConversationIgnored,
  setPaneIgnored,
  isOutsideFleet,
  needsRestoreInto,
  pickableProjects,
  projectLabel,
  proposalOf,
  showsUnsure,
} from './lost_found';
import { preselect } from './ai_proposal';
import { session } from './hosts_fixture';
import type { ProjectRow } from './projects';
import type { LostCandidate } from './sessions';

function project(id: number, repo: string, over: Partial<ProjectRow> = {}): ProjectRow {
  return {
    id,
    owner: 'acme',
    repo,
    base_path: `/p/acme/${repo}`,
    last_session_at: null,
    adopted: false,
    system: false,
    ...over,
  };
}

function candidate(over: Partial<LostCandidate> = {}): LostCandidate {
  return {
    cwd: '/home/ada/tmp',
    git_branch: null,
    claude_session_id: 'cs-a',
    transcript_mtime: 1,
    derived_tmux_name: null,
    project_id: null,
    worktree_id: null,
    existing_session_id: null,
    rank_hint: 'stale',
    resumable: false,
    ...over,
  };
}

const PROJECTS = [project(1, 'papaya-pos'), project(2, 'payments-api')];

describe('lost and found (4.12)', () => {
  it('a pane fleet did not start is outside fleet; a started, lost or paneless one is not', () => {
    expect(isOutsideFleet(session('h', 'scratch', { started_at: null }))).toBe(true);
    expect(isOutsideFleet(session('h', 'dev', { started_at: 5 }))).toBe(false);
    expect(isOutsideFleet(session('h', 'gone', { started_at: null, status: 'ghost' }))).toBe(false);
    expect(isOutsideFleet(session('h', 'lost', { started_at: null, lost_at: 9 }))).toBe(false);
    expect(isOutsideFleet(session('h', 'bg', { started_at: null, kind: 'bg' }))).toBe(false);
    expect(isOutsideFleet(session('h', 'ext', { started_at: null, kind: 'external' }))).toBe(false);
  });

  it('only a conversation with no row that cannot resume where it ran needs Restore into', () => {
    expect(needsRestoreInto(candidate())).toBe(true);
    expect(needsRestoreInto(candidate({ project_id: 1 }))).toBe(true);
    expect(
      needsRestoreInto(candidate({ project_id: 1, resumable: true, derived_tmux_name: 'dev-acme-papaya-pos' })),
    ).toBe(false);
    expect(needsRestoreInto(candidate({ existing_session_id: 4 }))).toBe(false);
  });

  it('offers the person’s projects, most recently used first, never fleet’s own', () => {
    const rows = [
      project(1, 'old', { last_session_at: 10 }),
      project(2, 'new', { last_session_at: 99 }),
      project(3, 'operator', { system: true, last_session_at: 1000 }),
      project(4, 'never'),
    ];
    expect(pickableProjects(rows).map((p) => p.id)).toEqual([2, 1, 4]);
    expect(projectLabel(rows[0])).toBe('acme/old');
  });

  it('a proposal pre-selects through the 3.11 floor; a rule always does', () => {
    const jev = proposalOf({ project_id: 1, source: 'jev', confidence_pct: 82, reason: 'directory' }, PROJECTS);
    expect(jev).toEqual({ value: '1', source: 'jev', reason: 'directory', confidence_pct: 82 });
    expect(preselect('project', jev)).toBe('1');
    const weak = proposalOf({ project_id: 1, source: 'jev', confidence_pct: 30 }, PROJECTS);
    expect(preselect('project', weak)).toBeNull();
    const rule = proposalOf({ project_id: 2, source: 'rule' }, PROJECTS);
    expect(preselect('project', rule)).toBe('2');
  });

  it('nothing is proposed for an empty target or a project the list does not hold', () => {
    expect(proposalOf(null, PROJECTS)).toBeNull();
    expect(proposalOf({}, PROJECTS)).toBeNull();
    expect(proposalOf({ project_id: 9, source: 'jev', confidence_pct: 90 }, PROJECTS)).toBeNull();
  });

  it('says Jev was unsure only when it was asked and named nothing', () => {
    expect(showsUnsure({ unsure: true })).toBe(true);
    expect(showsUnsure({})).toBe(false);
    expect(showsUnsure(null)).toBe(false);
    expect(UNSURE_NOTE).toBe('Jev was unsure, so nothing is filled in');
  });

  it('the confirmation names the entry and the project', () => {
    expect(confirmTitle('Adopt', 'fleet-trn-scratch', PROJECTS[0])).toBe(
      'Adopt fleet-trn-scratch into acme/papaya-pos?',
    );
    expect(confirmTitle('Adopt', 'scratch', null)).toBe('Adopt scratch without a project?');
  });
});

describe('Ignore a found conversation (G2.7)', () => {
  it('is kept per host on this device, and Bring back undoes it', () => {
    localStorage.clear();
    expect(ignoredConversations('mercury').size).toBe(0);
    setConversationIgnored('mercury', 'c-1', true);
    setConversationIgnored('mercury', 'c-2', true);
    setConversationIgnored('venus', 'c-1', true);
    expect([...ignoredConversations('mercury')].sort()).toEqual(['c-1', 'c-2']);
    setConversationIgnored('mercury', 'c-1', false);
    expect([...ignoredConversations('mercury')]).toEqual(['c-2']);
    expect([...ignoredConversations('venus')]).toEqual(['c-1']);
    localStorage.clear();
  });
});

describe('Ignore an outside pane and Find another (G4.5)', () => {
  it('keeps ignored panes per host on this device', () => {
    localStorage.clear();
    setPaneIgnored('mercury', 'scratch', true);
    setPaneIgnored('venus', 'scratch', true);
    expect([...ignoredPanes('mercury')]).toEqual(['scratch']);
    setPaneIgnored('mercury', 'scratch', false);
    expect(ignoredPanes('mercury').size).toBe(0);
    expect([...ignoredPanes('venus')]).toEqual(['scratch']);
    localStorage.clear();
  });

  it('offers the other conversations, its own project first', () => {
    const list = [
      candidate({ claude_session_id: 'own', project_id: 1 }),
      candidate({ claude_session_id: 'elsewhere', project_id: 2 }),
      candidate({ claude_session_id: 'same', project_id: 1 }),
    ];
    const ids = (l: LostCandidate[]) => l.map((c) => c.claude_session_id);
    expect(ids(otherConversations(list, { claude_session_id: 'own', project_id: 1 }))).toEqual(['same', 'elsewhere']);
    expect(ids(otherConversations(list, { claude_session_id: null, project_id: null }))).toEqual([
      'own',
      'elsewhere',
      'same',
    ]);
  });
});
