// The Work view in the app shell (work graph M14): the sidebar's
// Sessions | Work switch, and Details showing a task picked in the Work
// view until a session is opened.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import Sidebar from './Sidebar.svelte';
import Details from './Details.svelte';
import { sessions } from './sessions';
import { clearSelection, selectSessionExplicitly } from './selection';
import { session } from './hosts_fixture';
import { openTask, selectedTaskId, sidebarView, taskDetailOpen } from './work_view';
import { onboardingDismissed } from './onboarding';
import { clearSessionFocus, focusSession } from './session_focus';

async function flush() {
  for (let i = 0; i < 8; i++) await tick();
}

describe('Sessions | Work switch', () => {
  beforeEach(() => {
    sidebarView.set('sessions');
    sessions.set([]);
    onboardingDismissed.set(true);
  });

  it('shows the Sessions tree by default and the Work tree when chosen', async () => {
    render(Sidebar, { onCollapse: () => {} });
    await flush();
    expect(screen.getByTestId('sidebar-search')).toBeTruthy();
    expect(screen.queryByTestId('work-tree')).toBeNull();
    // The rail picks the view (redesign 3.2); the list only names it.
    expect(screen.queryByTestId('sidebar-view-switch')).toBeNull();
    expect(screen.getByTestId('list-title').textContent).toBe('All sessions');
    sidebarView.set('work');
    await flush();
    expect(screen.getByTestId('list-title').textContent).toBe('Work');
    expect(screen.getByTestId('work-tree')).toBeTruthy();
    expect(screen.queryByTestId('sidebar-search')).toBeNull();
    // The footer (New session) stays in both.
    expect(screen.getByTestId('new-session-head')).toBeTruthy();
    // So does the global chrome: Refresh and collapse (Settings is on the rail, ⌘,).
    expect(screen.queryByTestId('settings-open')).toBeNull();
    for (const id of ['sidebar-refresh', 'sidebar-collapse']) {
      expect(screen.getByTestId(id)).toBeTruthy();
    }
    // The ☑ Tasks popover is gone: delegated jobs live in the Work list.
    expect(screen.queryByTestId('tasks-open')).toBeNull();
    // The Sessions list's own filters step aside: none of them narrows the
    // Work tree, which has its own.
    for (const id of ['needs-you-filter', 'select-mode', 'filters-open', 'scope-select']) {
      expect(screen.queryByTestId(id)).toBeNull();
    }
    // One refresh, not two.
    expect(screen.queryByTestId('work-refresh')).toBeNull();
    // One collapse control, not two.
    expect(screen.queryByTestId('work-collapse')).toBeNull();
    sidebarView.set('sessions');
    await flush();
    expect(screen.getByTestId('sidebar-search')).toBeTruthy();
    expect(screen.queryByTestId('work-tree')).toBeNull();
  });

  it('the Needs you count is on the rail (Inbox) and in the filter panel, not on a list switch', async () => {
    sessions.set([session('mefistos', 'api', { id: 7, stuck_kind: 'oom' })]);
    sidebarView.set('work');
    render(Sidebar, { onCollapse: () => {} });
    await flush();
    expect(screen.queryByTestId('sessions-tab-needs-you')).toBeNull();
    sidebarView.set('sessions');
    await flush();
    // The Needs you pill lives in the filter panel (step 3.7).
    await fireEvent.click(screen.getByTestId('filters-open'));
    await flush();
    expect(screen.getByTestId('needs-you-filter').textContent).toMatch(/Needs you\s1/);
  });

  it('a focused session brings the Sessions list back', async () => {
    sessions.set([session('mefistos', 'api', { id: 7 })]);
    sidebarView.set('work');
    render(Sidebar, { onCollapse: () => {} });
    await flush();
    focusSession(7, 'api');
    await flush();
    expect(get(sidebarView)).toBe('sessions');
    expect(screen.getByTestId('session-focus-bar')).toBeTruthy();
    clearSessionFocus();
  });
});

describe('Details with the Work view', () => {
  beforeEach(() => {
    clearSelection();
    sidebarView.set('work');
    selectedTaskId.set(null);
    taskDetailOpen.set(false);
    sessions.set([session('mefistos', 'api', { id: 7 })]);
  });

  it('a picked task shows until a session is opened; Close goes back', async () => {
    render(Details);
    await flush();
    expect(screen.queryByTestId('work-task-detail')).toBeNull();
    openTask('item:12');
    await flush();
    expect(screen.getByTestId('work-task-detail')).toBeTruthy();
    selectSessionExplicitly(get(sessions)[0]);
    await flush();
    expect(screen.queryByTestId('work-task-detail')).toBeNull();
    expect(get(taskDetailOpen)).toBe(false);
    // The task stays selected in the tree.
    expect(get(selectedTaskId)).toBe('item:12');
    openTask('item:12');
    await flush();
    expect(screen.getByTestId('work-task-close').textContent).toContain('Back to api');
    await fireEvent.click(screen.getByTestId('work-task-close'));
    await flush();
    expect(screen.queryByTestId('work-task-detail')).toBeNull();
  });

  it('only in the Work view', async () => {
    sidebarView.set('sessions');
    render(Details);
    openTask('item:12');
    await flush();
    expect(screen.queryByTestId('work-task-detail')).toBeNull();
  });
});
