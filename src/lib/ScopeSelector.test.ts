// Work graph M5.4: the scope selector (only at two or more scopes) and the
// needs-you line for other scopes.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));

import SidebarFilters from './SidebarFilters.svelte';
import { sessions, type SessionRow } from './sessions';
import { projects, type ProjectTreeRow } from './projects';
import { orgs, scopeFilter } from './orgs';

let nextId = 1;
function row(over: Partial<SessionRow>): SessionRow {
  return {
    id: nextId++, tmux_name: 'dev', host_alias: 'h1', project_id: 1, worktree_id: null, created_at: 1,
    last_activity_at: 1, status: 'running', notes: null, account_uuid: null, kind: 'work',
    reviews_session_id: null, worktree_key: 'main', lost_at: null, claude_session_id: null,
    claude_status: null, effort_level: null, pr_url: null, current_activity: null, context_pct: null,
    stuck_kind: null, friendly_name: null, safe_kill_state: null, safe_kill_nonce: null,
    safe_kill_detail: null, safe_kill_requested_at: null, idle_since: null, stuck_since: null,
    last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null,
    turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null,
    context_tokens: null, context_window: null, context_source: null, context_at: null,
    context_stale: false, tmux_pane_id: null, pending_input: null,
    ...over,
  } as SessionRow;
}
const project = (id: number, owner: string) =>
  ({ project: { id, owner, repo: `r${id}`, base_path: `/p${id}`, last_session_at: null, adopted: false, system: false }, worktrees: [] }) as ProjectTreeRow;

const props = {
  search: '',
  recency: 'all' as never,
  needsYouOnly: false,
  loading: false,
  loadError: null,
  onRefresh: () => {},
  showTasks: false,
  showSettings: false,
  onOpenTasks: () => {},
  onOpenSettings: () => {},
  needsYouCount: 0,
  selectMode: false,
  toggleSelectMode: () => {},
  selectedCount: 0,
  onBulkSend: () => {},
  onBulkKill: () => {},
  clearSelected: () => {},
};

beforeEach(() => {
  sessions.set([]);
  projects.set([project(1, 'acme'), project(2, 'beta')]);
  orgs.set([]);
  scopeFilter.set('all');
});

describe('the scope selector', () => {
  it('is absent with one scope — a single-company fleet sees no new chrome', () => {
    sessions.set([row({ project_id: 1 })]);
    render(SidebarFilters, { props });
    expect(screen.queryByTestId('scope-select')).toBeNull();
  });

  it('appears with two scopes and sets the scope', async () => {
    sessions.set([row({ project_id: 1 }), row({ project_id: 2 })]);
    render(SidebarFilters, { props });
    const sel = (await screen.findByTestId('scope-select')) as HTMLSelectElement;
    expect(Array.from(sel.options).map((o) => o.textContent)).toEqual(['All', 'acme', 'beta', 'Unassigned']);
    await fireEvent.change(sel, { target: { value: 'owner:beta' } });
    expect(get(scopeFilter)).toBe('owner:beta');
  });

  it('never hides needs-you: another scope\'s waiting session gets a line that switches to it', async () => {
    orgs.set([
      { id: 1, name: 'Company A', created_at: 1, rules: [], hosts: [], trackers: [] },
      { id: 2, name: 'Personal', created_at: 1, rules: [], hosts: [], trackers: [] },
    ]);
    sessions.set([row({ org_id: 1 }), row({ org_id: 2, claude_status: 'blocked' })]);
    scopeFilter.set('org:1');
    render(SidebarFilters, { props });
    const item = await screen.findByTestId('needs-you-elsewhere-item');
    expect(item.textContent).toContain('1 needs you in Personal →');
    await fireEvent.click(item);
    await waitFor(() => expect(get(scopeFilter)).toBe('org:2'));
    await waitFor(() => expect(screen.queryByTestId('needs-you-elsewhere')).toBeNull());
  });
});
