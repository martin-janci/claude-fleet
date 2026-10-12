import { describe, it, expect } from 'vitest';
import { get } from 'svelte/store';
import {
  agentFilter,
  agentFilterLabel,
  inScopeTab,
  isSharedAccess,
  personName,
  scopeTab,
  scopeTabCounts,
  sharedByLine,
} from './session_scope';
import { session } from './hosts_fixture';
import type { SessionAccess } from './access';
import type { OrgDetail } from './orgs';

const org = (members: { person_id: number; name: string; display_name?: string }[]) =>
  ({ id: 1, name: 'Acme', members: members.map((m) => ({ ...m, role: 'member' })) }) as unknown as OrgDetail;

describe('scope tabs (Sessions board: All / Mine / Shared with me)', () => {
  it('Mine holds what this person owns; Shared every grant level; All everything', () => {
    const levels: SessionAccess[] = ['own', 'watch', 'answer', 'drive', null];
    expect(levels.map((a) => inScopeTab('mine', a))).toEqual([true, false, false, false, false]);
    expect(levels.map((a) => inScopeTab('shared', a))).toEqual([false, true, true, true, false]);
    expect(levels.map((a) => inScopeTab('all', a))).toEqual([true, true, true, true, true]);
    expect(levels.map(isSharedAccess)).toEqual([false, true, true, true, false]);
  });

  it('counts each tab; an unknown access counts under All only', () => {
    const rows = [session('h', 'a'), session('h', 'b'), session('h', 'c'), session('h', 'd')];
    const access = new Map<number, SessionAccess>([
      [rows[0].id, 'own'],
      [rows[1].id, 'watch'],
      [rows[2].id, 'drive'],
      [rows[3].id, null],
    ]);
    expect(scopeTabCounts(rows, (s) => access.get(s.id) ?? null)).toEqual({ all: 4, mine: 1, shared: 2 });
  });

  it('remembers the tab and the agent across launches', () => {
    scopeTab.set('shared');
    expect(JSON.parse(localStorage.getItem('cf:pref:sessions.scope-tab')!)).toBe('shared');
    agentFilter.set('codex');
    expect(JSON.parse(localStorage.getItem('cf:pref:sessions.agent-filter')!)).toBe('codex');
    scopeTab.set('all');
    agentFilter.set('any');
    expect(get(agentFilter)).toBe('any');
  });
});

describe('a shared row’s line', () => {
  it('names the sharer from an org’s members, display name first', () => {
    const orgs = [org([{ person_id: 9, name: 'petra', display_name: 'Petra' }, { person_id: 4, name: 'tomas' }])];
    expect(personName(9, orgs)).toBe('Petra');
    expect(personName(4, orgs)).toBe('tomas');
    expect(personName(5, orgs)).toBeNull();
    expect(personName(null, orgs)).toBeNull();
  });

  it('says the level in the board’s words', () => {
    const orgs = [org([{ person_id: 9, name: 'petra', display_name: 'Petra' }])];
    expect(sharedByLine({ owner_person_id: 9 }, 'watch', orgs)).toBe('Shared by Petra · can watch');
    expect(sharedByLine({ owner_person_id: 9 }, 'drive', orgs)).toBe('Shared by Petra · can steer');
    expect(sharedByLine({ owner_person_id: 2 }, 'answer', orgs)).toBe('Shared with you · can answer');
  });

  it('labels the agent facet', () => {
    expect(agentFilterLabel('any')).toBe('Any');
    expect(agentFilterLabel('claude')).toBe('Claude Code');
  });
});
