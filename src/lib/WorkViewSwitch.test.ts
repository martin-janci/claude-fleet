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
import { todayOpen } from './today';
import { openTask, selectedTaskId, sidebarView, taskDetailOpen } from './work_view';
import { onboardingDismissed } from './onboarding';

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
    await fireEvent.click(screen.getByTestId('sidebar-view-work'));
    await flush();
    expect(get(sidebarView)).toBe('work');
    expect(screen.getByTestId('work-tree')).toBeTruthy();
    expect(screen.queryByTestId('sidebar-search')).toBeNull();
    // The footer (New session) stays in both.
    expect(screen.getByTestId('new-session-footer')).toBeTruthy();
    // So does the shared chrome: Refresh, Needs you, select, Tasks, Settings.
    for (const id of ['sidebar-refresh', 'needs-you-filter', 'select-mode', 'tasks-open', 'settings-open', 'sidebar-collapse']) {
      expect(screen.getByTestId(id)).toBeTruthy();
    }
    // One collapse control, not two.
    expect(screen.queryByTestId('work-collapse')).toBeNull();
    await fireEvent.click(screen.getByTestId('sidebar-view-sessions'));
    await flush();
    expect(screen.getByTestId('sidebar-search')).toBeTruthy();
    expect(screen.queryByTestId('work-tree')).toBeNull();
  });

  it('Needs you in the Work view goes to the Sessions list with the filter on', async () => {
    sidebarView.set('work');
    render(Sidebar, { onCollapse: () => {} });
    await flush();
    const pill = screen.getByTestId('needs-you-filter');
    expect(pill.getAttribute('aria-pressed')).toBe('false');
    await fireEvent.click(pill);
    await flush();
    expect(get(sidebarView)).toBe('sessions');
    expect(screen.getByTestId('needs-you-filter').getAttribute('aria-pressed')).toBe('true');
    expect(screen.queryByTestId('work-tree')).toBeNull();
  });
});

describe('Details with the Work view', () => {
  beforeEach(() => {
    clearSelection();
    todayOpen.set(false);
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
