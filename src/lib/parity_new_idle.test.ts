// Parity P9 (docs/redesign/archive/parity.md): the idle-too-long nudge, under the New
// layout. 0.5.4's Needs you filter also showed sessions idle past the
// threshold; in New the toggle moved into the Filters panel (step 3.7), and
// the row must still come through it, shown by the filter but not counted in
// the pill (attention.test.ts › counts narrower than it filters).
import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

import Sidebar from './Sidebar.svelte';
import { projects, type ProjectTreeRow } from './projects';
import { sessions, showBgAgents, showRowDetails, resetTombstonesForTests, type SessionRow } from './sessions';
import { hosts, hostFilter, resetTombstonesForTests as resetHostTombstones } from './hosts';
import { accounts } from './accounts';
import { selectSession } from './selection';
import { sessionFocus } from './session_focus';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { resetAccessForTests } from './access';
import { onboardingDismissed } from './onboarding';
import { classify } from './attention';

const NOW = Math.floor(Date.now() / 1000);

const fakeProjects = [
  {
    project: { id: 1, owner: 'o', repo: 'alpha', base_path: '/r/a', last_session_at: NOW - 60, adopted: false, system: false },
    worktrees: [{ id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/r/a', branch: 'main' }],
  },
  {
    project: { id: 2, owner: 'o', repo: 'beta', base_path: '/r/b', last_session_at: NOW - 60, adopted: false, system: false },
    worktrees: [{ id: 21, project_id: 2, host_alias: 'local', name: 'main', path: '/r/b', branch: 'main' }],
  },
] as unknown as ProjectTreeRow[];

let nextId = 5000;
function row(projectId: number, name: string, over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: nextId++, tmux_name: name, host_alias: 'local', project_id: projectId, worktree_id: null, created_at: 1,
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

beforeEach(() => {
  localStorage.clear();
  resetTombstonesForTests();
  resetHostTombstones();
  hosts.set([]);
  accounts.set([]);
  hostFilter.set('all');
  showBgAgents.set(true);
  showRowDetails.set(true);
  selectSession(null);
  sessionFocus.set(null);
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  resetAccessForTests();
  onboardingDismissed.set(true);
});
afterEach(() => {
  projects.set([]);
  sessions.set([]);
});

describe('Needs you in the New layout', () => {
  it('New layout: an idle-too-long session comes through the Needs you toggle in the Filters panel, shown but not counted', async () => {
    // Idle since the epoch: far past any idle threshold.
    const idle = row(1, 'dev-idle-long', { claude_status: 'idle', idle_since: 1 });
    const blocked = row(1, 'dev-blocked', { claude_status: 'blocked' });
    const working = row(2, 'dev-working', { claude_status: 'working', last_activity_at: NOW });
    expect(classify(idle, { idleSecs: 1800, now: NOW })).toBe('idle_long');
    projects.set(fakeProjects);
    sessions.set([idle, blocked, working]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getAllByTestId('sess-row')).toHaveLength(3);
    // New draws no Needs you pill on the row; the toggle is in the panel.
    expect(screen.queryByTestId('needs-you-filter')).toBeNull();
    await fireEvent.click(screen.getByTestId('filters-open'));
    await tick();
    const toggle = within(screen.getByTestId('filter-panel')).getByTestId('needs-you-filter');
    // The count is the blocked row alone: idle rows never inflate it.
    expect(toggle).toHaveTextContent('Needs you 1');
    await fireEvent.click(toggle);
    await tick(); await tick();
    expect(toggle.getAttribute('aria-pressed')).toBe('true');
    const names = screen.getAllByTestId('sess-row').map((r) => r.textContent ?? '');
    expect(names).toHaveLength(2);
    expect(names.some((n) => n.includes('dev-idle-long'))).toBe(true);
    expect(names.some((n) => n.includes('dev-blocked'))).toBe(true);
    expect(names.some((n) => n.includes('dev-working'))).toBe(false);
    expect(screen.getByTestId('active-filters').textContent).toContain('Needs you');
  });
});
