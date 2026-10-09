import { render, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => { throw { code: 'E_TEST', message: 'no backend' }; }) }));
import ControlViewsPanel from './ControlViewsPanel.svelte';
import ControlView from './ControlView.svelte';
import { controlViews, defaultLayout } from './control_views';
import { sessions } from './sessions';
import { selectedSession, onSessionOpened } from './selection';
import { destination } from './destination';
import { sidebarView } from './work_view';
import { session } from './hosts_fixture';
import { expectAccessible } from './a11y_check';

// Redesign step 9.4: the Views panel beside Control's chat.

const asking = session('mac', 'asking', { claude_status: 'blocked', last_prompt: 'Fix the login bug\nthen ship' });
const busy = session('mac', 'busy', { claude_status: 'working' });

beforeEach(() => {
  controlViews.set(defaultLayout());
  sessions.set([asking, busy]);
});
afterEach(() => {
  sessions.set([]);
  destination.set('session');
});

describe('ControlViewsPanel (step 9.4)', () => {
  it('opens on Needs you, listing what the Inbox lists, with its count on the tab', () => {
    const { getByTestId, getAllByTestId } = render(ControlViewsPanel);
    expect(getByTestId('control-view-tab-needs-you').getAttribute('aria-selected')).toBe('true');
    expect(getByTestId('control-view-tab-needs-you').textContent).toContain('1');
    const rows = getAllByTestId('control-needs-you-row');
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain('asking');
  });

  it('a Needs you row puts that session in focus, in the panel', async () => {
    const { getByTestId } = render(ControlViewsPanel);
    await fireEvent.click(getByTestId('control-needs-you-row'));
    expect(get(selectedSession)?.id).toBe(asking.id);
    expect(get(controlViews).active).toBe('session');
    expect(getByTestId('control-session-focus').textContent).toContain('Fix the login bug');
  });

  it('a Needs you row keeps you in Control: nothing is told to navigate (UX audit 2026-10-09, C2)', async () => {
    destination.set('control');
    const opened = vi.fn();
    const off = onSessionOpened(opened);
    const { getByTestId } = render(ControlViewsPanel);
    await fireEvent.click(getByTestId('control-needs-you-row'));
    off();
    expect(opened).not.toHaveBeenCalled();
    expect(get(destination)).toBe('control');
    expect(get(selectedSession)?.id).toBe(asking.id);
  });

  it('the session opens here with its conversation; ‹ and Esc go back, ⤢ opens it in full (step 9.5)', async () => {
    destination.set('control');
    const { getByTestId, queryByTestId } = render(ControlViewsPanel);
    await fireEvent.click(getByTestId('control-needs-you-row'));
    expect(getByTestId('control-session-back').textContent).toContain('Needs you');
    await fireEvent.click(getByTestId('control-session-back'));
    expect(get(controlViews).active).toBe('needs-you');
    expect(queryByTestId('control-session-focus')).toBeNull();

    await fireEvent.click(getByTestId('control-needs-you-row'));
    await fireEvent.keyDown(getByTestId('control-session-focus'), { key: 'Escape' });
    expect(get(controlViews).active).toBe('needs-you');

    await fireEvent.click(getByTestId('control-needs-you-row'));
    await fireEvent.click(getByTestId('control-views-expand'));
    expect(get(destination)).toBe('session');
    expect(get(selectedSession)?.id).toBe(asking.id);
  });

  it('"+" turns a view off and links out to where the others live', async () => {
    const { getByTestId, queryByTestId } = render(ControlViewsPanel);
    await fireEvent.click(getByTestId('control-views-add'));
    expect(getByTestId('control-views-menu').textContent).toContain('Open elsewhere');
    await fireEvent.click(getByTestId('control-views-toggle-prs'));
    expect(queryByTestId('control-view-tab-prs')).toBeNull();
    destination.set('control');
    await fireEvent.click(getByTestId('control-views-elsewhere-missions'));
    expect(get(sidebarView)).toBe('work');
    expect(get(destination)).toBe('session');
  });

  it('⤢ opens Needs you as the Inbox', async () => {
    destination.set('control');
    const { getByTestId } = render(ControlViewsPanel);
    await fireEvent.click(getByTestId('control-views-expand'));
    expect(get(sidebarView)).toBe('inbox');
    expect(get(destination)).toBe('session');
  });

  it('✕ closes the column and Views opens it again', async () => {
    const { getByTestId, queryByTestId } = render(ControlView, { isMac: false });
    await fireEvent.click(getByTestId('control-views-close'));
    expect(queryByTestId('control-views')).toBeNull();
    await fireEvent.click(getByTestId('control-views-open'));
    expect(getByTestId('control-views')).toBeTruthy();
  });

  it('passes the axe and audit checks', async () => {
    const { container, getByTestId } = render(ControlViewsPanel);
    await fireEvent.click(getByTestId('control-views-add'));
    await expectAccessible(container);
  });
});
