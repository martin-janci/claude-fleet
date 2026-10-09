// Redesign step 3.1: one destination store owns the right column: never two
// overlays at once, and the terminal underneath stays mounted through every
// overlay round trip.
import { render, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, beforeAll, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import App from './App.svelte';
import { onboardingDismissed } from './lib/onboarding';
import { clearToasts } from './lib/toasts';
import { workBoardOpen, requestHostsView, settingsOpen, requestAssetsView, shortcutSheetOpen } from './lib/app_views';
import { toolkitTab } from './lib/toolkit_skills';
import { sidebarView } from './lib/work_view';
import { destination } from './lib/destination';
import { controlTab } from './lib/control';
import { link, task } from './lib/work_view_fixture';
import type { WorkTreePage } from './lib/work_view';
import { activeHintId, hintDef, markSeen, resetHints } from './lib/hints';
import { selectSessionExplicitly } from './lib/selection';
import { session } from './lib/hosts_fixture';
import { onboardingWelcomed } from './lib/onboarding';
import { preloadLazyViews } from './lib/lazy_views';

// The off-screen views load lazily in the app; here they are in place
// before the first render, so a test sees them on the frame they open.
beforeAll(() => preloadLazyViews());

const OVERLAYS = ['hosts-overlay', 'assets-overlay', 'board-view', 'accounts-overlay', 'control-overlay', 'automation-overlay'];

beforeEach(() => {
  onboardingDismissed.set(true);
  clearToasts();
});
afterEach(() => {
  destination.set('session');
  sidebarView.set('sessions');
  settingsOpen.set(false);
  controlTab.set('chat');
});

function terminalSlot(container: HTMLElement): Element {
  const slot = container.querySelector('.right-body > .view-slot:not(.overlay)');
  expect(slot).not.toBeNull();
  return slot!.firstElementChild!;
}

function openOverlays(container: HTMLElement): string[] {
  return OVERLAYS.filter((id) => container.querySelector(`[data-testid="${id}"]`));
}

describe('App: the destination store', () => {
  it('starts on the Session tab', () => {
    const { container, getByTestId } = render(App);
    expect(get(destination)).toBe('session');
    expect(openOverlays(container)).toEqual([]);
    expect(getByTestId('rail-sessions').getAttribute('aria-current')).toBe('page');
  });

  it('keeps the terminal mounted, and one overlay at most, through every overlay', async () => {
    const { container, getByTestId } = render(App);
    const term = terminalSlot(container);

    await fireEvent.click(getByTestId('rail-toolkit'));
    expect(openOverlays(container)).toEqual(['assets-overlay']);

    requestHostsView();
    await waitFor(() => expect(get(destination)).toBe('hosts'));
    expect(openOverlays(container)).toEqual(['hosts-overlay']);

    workBoardOpen.set(true);
    await waitFor(() => expect(openOverlays(container)).toEqual(['board-view']));

    await fireEvent.click(getByTestId('rail-accounts'));
    expect(openOverlays(container)).toEqual(['accounts-overlay']);
    await fireEvent.click(getByTestId('rail-sessions'));
    expect(get(destination)).toBe('session');
    expect(openOverlays(container)).toEqual([]);
    expect(terminalSlot(container)).toBe(term);
  });

  it('a Hosts request from outside App replaces the open overlay', async () => {
    const { container, getByTestId } = render(App);
    await fireEvent.click(getByTestId('rail-toolkit'));
    requestHostsView();
    await waitFor(() => expect(openOverlays(container)).toEqual(['hosts-overlay']));
    expect(get(destination)).toBe('hosts');
  });

  it('the board is a Work view, with no close and no Esc', async () => {
    const { container, queryByTestId } = render(App);
    workBoardOpen.set(true);
    await waitFor(() => expect(queryByTestId('board-view')).not.toBeNull());
    expect(queryByTestId('work-board-close')).toBeNull();
    await fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(get(destination)).toBe('board');
    // The terminal stays mounted under it, as under every destination.
    expect(container.querySelector('.right-body > .view-slot:not(.overlay)')).not.toBeNull();
  });

  // The board (parity P3, P26), driven through App, with a native and a
  // tracker card served.
  const boardPage = (): WorkTreePage => ({
    tasks: [
      task({
        task_id: 'item:1',
        item_id: 1,
        key: 'TASK-1',
        title: 'Write notes',
        kind: 'local',
        origin: 'manual',
        status_category: 'todo',
        status_name: null,
        counts: { active: 0, ended: 0, suggested: 0 },
        sessions: [],
      }),
      task({
        task_id: 'item:12',
        item_id: 12,
        key: 'ABC-12',
        title: 'Login fails',
        status_category: 'in_progress',
        sessions: [link({ name: 'abc-12 login', host: 'mefistos' })],
      }),
    ],
    groups: [],
    orgs: [],
    trackers: [],
    total: 2,
    next_cursor: null,
  });
  async function withBoardBackend(body: () => Promise<void>) {
    const mock = vi.mocked(invoke);
    const fallback = mock.getMockImplementation()!;
    mock.mockImplementation(async (cmd: string, payload?: unknown) => {
      if (cmd === 'work_tree') return boardPage();
      if (cmd === 'set_work_status') {
        const a = (payload as { args: { item_id: number; status: string } }).args;
        return { id: a.item_id, source: 'local', key: 'TASK-1', title: 'Write notes', status_category: a.status };
      }
      return fallback(cmd, payload as never);
    });
    try {
      await body();
    } finally {
      mock.mockImplementation(fallback);
    }
  }

  it('on the board, ← → move a focused native card across columns and e opens its edit dialog', async () => {
    await withBoardBackend(async () => {
      const { getByTestId, queryByTestId, getAllByTestId } = render(App);
      workBoardOpen.set(true);
      await waitFor(() => expect(queryByTestId('board-view')).not.toBeNull());
      const card = (title: string) =>
        getAllByTestId('work-board-card').find((el) => el.textContent?.includes(title)) as HTMLElement;
      const col = (c: string) => getByTestId(`work-board-column-${c}`);
      await waitFor(() => expect(col('todo').textContent).toContain('Write notes'));
      const sets = () => vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'set_work_status');
      const before = sets().length;

      await fireEvent.keyDown(card('Write notes'), { key: 'ArrowRight' });
      await waitFor(() => expect(col('doing').textContent).toContain('Write notes'));
      expect(sets().slice(before)[0][1]).toEqual({ args: { item_id: 1, status: 'in_progress' } });

      await fireEvent.keyDown(card('Write notes'), { key: 'ArrowLeft' });
      await waitFor(() => expect(col('todo').textContent).toContain('Write notes'));
      expect(sets().slice(before)[1][1]).toEqual({ args: { item_id: 1, status: 'todo' } });

      // e on a tracker card opens nothing; on a native card, the edit dialog.
      await fireEvent.keyDown(card('Login fails'), { key: 'e' });
      expect(queryByTestId('edit-task-dialog')).toBeNull();
      await fireEvent.keyDown(card('Write notes'), { key: 'e' });
      await waitFor(() => expect(queryByTestId('edit-task-dialog')).not.toBeNull());
    });
  });

  it('review r08: a board card opens its task beside the board', async () => {
    await withBoardBackend(async () => {
      const { getAllByTestId, queryByTestId } = render(App);
      workBoardOpen.set(true);
      await waitFor(() => expect(queryByTestId('board-view')).not.toBeNull());
      await waitFor(() => expect(getAllByTestId('work-board-card').length).toBe(2));
      expect(queryByTestId('work-task-detail')).toBeNull();
      const card = getAllByTestId('work-board-card').find((el) => el.textContent?.includes('Write notes'))!;
      await fireEvent.click(card);
      await waitFor(() => expect(queryByTestId('work-task-detail')).not.toBeNull());
      // The board stays: the task opens in the inspector column beside it.
      expect(queryByTestId('board-view')).not.toBeNull();
      expect(get(destination)).toBe('board');
    });
  });

  it('the board offers its one-time move hint, and not again once dismissed', async () => {
    resetHints();
    const welcomed = get(onboardingWelcomed);
    onboardingWelcomed.set(true);
    try {
      await withBoardBackend(async () => {
        const { getByTestId, queryByTestId } = render(App);
        workBoardOpen.set(true);
        await waitFor(() => expect(queryByTestId('board-view')).not.toBeNull());
        expect(queryByTestId('work-board-hint')).toBeNull();
        expect(getByTestId('work-board').querySelector('header')!.textContent).not.toContain('Drag a task');
        await waitFor(() => expect(get(activeHintId)).toBe('board-move'));
        expect(hintDef('board-move')!.text).toContain('Drag a task to set its status');
        markSeen('board-move');
        expect(get(activeHintId)).not.toBe('board-move');
      });
    } finally {
      resetHints();
      onboardingWelcomed.set(welcomed);
    }
  });

});

describe('App: the rail and the Accounts page (steps 3.2, 4.1)', () => {
  it('the rail shows the landed items in the manual order, Settings last', () => {
    const { getByTestId } = render(App);
    const ids = Array.from(getByTestId('rail').querySelectorAll('[data-testid^="rail-"]'), (e) =>
      e.getAttribute('data-testid'),
    );
    expect(ids).toEqual([
      'rail-control',
      'rail-inbox',
      'rail-sessions',
      'rail-work',
      'rail-automation',
      'rail-accounts',
      'rail-toolkit',
      'rail-settings',
    ]);
    expect(getByTestId('rail-sessions').getAttribute('aria-current')).toBe('page');
  });

  it('Accounts opens as one more overlay over a mounted terminal, and Esc leaves it', async () => {
    const { container, getByTestId, queryByTestId } = render(App);
    const term = terminalSlot(container);
    // The tab bar has no Hosts tab (step 3.5): ⌘I and the rail.
    requestHostsView();
    await waitFor(() => expect(getByTestId('rail-accounts').getAttribute('aria-current')).toBe('page'));
    await fireEvent.click(getByTestId('rail-accounts'));
    expect(openOverlays(container)).toEqual(['accounts-overlay']);
    expect(getByTestId('rail-accounts').getAttribute('aria-current')).toBe('page');
    // Accounts takes the whole width: no session tabs, no list column (UX audit N1, N2).
    expect(queryByTestId('session-tabs')).toBeNull();
    expect(container.querySelector('main.layout')?.classList.contains('list-off')).toBe(true);
    await fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(get(destination)).toBe('session');
    expect(terminalSlot(container)).toBe(term);
  });

  it('Automation (step 8.4) opens from the rail over a mounted terminal, and Esc leaves it', async () => {
    const { container, getByTestId } = render(App);
    const term = terminalSlot(container);
    await fireEvent.click(getByTestId('rail-automation'));
    expect(openOverlays(container)).toEqual(['automation-overlay']);
    expect(getByTestId('rail-automation').getAttribute('aria-current')).toBe('page');
    expect(getByTestId('automation-view')).toBeTruthy();
    await fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(get(destination)).toBe('session');
    expect(terminalSlot(container)).toBe(term);
  });

  it('opening a session leaves every fleet page for it (review r07)', async () => {
    const { container, getByTestId } = render(App);
    const row = session('mefistos', 'dev-open');
    for (const page of ['accounts', 'control', 'automation']) {
      await fireEvent.click(getByTestId(`rail-${page}`));
      expect(openOverlays(container)).toEqual([`${page}-overlay`]);
      selectSessionExplicitly(row);
      await waitFor(() => expect(openOverlays(container)).toEqual([]));
      expect(get(destination)).toBe('session');
    }
  });

  it('Toolkit (step 3.16) is the Assets screen, from the rail and from every old entry point', async () => {
    toolkitTab.set('skills');
    const { container, getByTestId } = render(App);
    const term = terminalSlot(container);
    await fireEvent.click(getByTestId('rail-toolkit'));
    expect(openOverlays(container)).toEqual(['assets-overlay']);
    expect(getByTestId('rail-toolkit').getAttribute('aria-current')).toBe('page');
    expect(getByTestId('toolkit-skills')).toBeTruthy();
    await fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(get(destination)).toBe('session');
    expect(terminalSlot(container)).toBe(term);
    // The quick switcher's asset rows open the same screen on its Assets tab.
    requestAssetsView({ select: 'asset:personal:skill/x' });
    await waitFor(() => expect(openOverlays(container)).toEqual(['assets-overlay']));
    expect(get(toolkitTab)).toBe('assets');
    expect(getByTestId('toolkit')).toBeTruthy();
  });

  it('the status bar ends on Shortcuts (step 3.17), with the rest unchanged', async () => {
    const { getByTestId } = render(App);
    await fireEvent.click(getByTestId('footer-shortcuts'));
    expect(get(shortcutSheetOpen)).toBe(true);
    shortcutSheetOpen.set(false);
  });

  it('Work and Sessions pick the sidebar tree and leave a fleet page', async () => {
    const { getByTestId } = render(App);
    await fireEvent.click(getByTestId('rail-accounts'));
    await fireEvent.click(getByTestId('rail-work'));
    expect(get(sidebarView)).toBe('work');
    expect(get(destination)).toBe('session');
    expect(getByTestId('rail-work').getAttribute('aria-current')).toBe('page');
    await fireEvent.click(getByTestId('rail-sessions'));
    expect(get(sidebarView)).toBe('sessions');
    expect(getByTestId('rail-sessions').getAttribute('aria-current')).toBe('page');
  });

  it('Inbox (step 3.3) shows the Inbox list, with no Today tab since 9.1', async () => {
    const { getByTestId, queryByTestId } = render(App);
    await fireEvent.click(getByTestId('rail-accounts'));
    await fireEvent.click(getByTestId('rail-inbox'));
    expect(get(sidebarView)).toBe('inbox');
    expect(get(destination)).toBe('session');
    expect(getByTestId('rail-inbox').getAttribute('aria-current')).toBe('page');
    expect(getByTestId('inbox')).toBeTruthy();
    expect(getByTestId('inbox-title')).toBeTruthy();
    expect(queryByTestId('inbox-tab-today')).toBeNull();
    // The way to everything else.
    await fireEvent.click(getByTestId('inbox-all-sessions'));
    expect(get(sidebarView)).toBe('sessions');
    expect(queryByTestId('inbox')).toBeNull();
    expect(getByTestId('list-title').textContent).toContain('All sessions');
  });

  it('Settings opens the Settings dialog', async () => {
    const { getByTestId } = render(App);
    await fireEvent.click(getByTestId('rail-settings'));
    expect(get(settingsOpen)).toBe(true);
    settingsOpen.set(false);
  });

  it('review r08: Settings opens from the rail while the sidebar is collapsed', async () => {
    const { getByTestId, queryByTestId } = render(App);
    await fireEvent.click(getByTestId('sidebar-collapse'));
    await waitFor(() => expect(queryByTestId('sidebar-expand')).not.toBeNull());
    await fireEvent.click(getByTestId('rail-settings'));
    await waitFor(() => expect(document.querySelector('.settings-dialog')).not.toBeNull());
    settingsOpen.set(false);
  });

});

describe('App: Control (step 9.1)', () => {
  it('the rail opens Control over a mounted terminal, with the agent in place of its sheet', async () => {
    const { container, getByTestId, queryByTestId } = render(App);
    const term = terminalSlot(container);
    // No floating agent button: the rail is the way in.
    expect(container.querySelector('.agent-fab')).toBeNull();
    await fireEvent.click(getByTestId('rail-control'));
    expect(openOverlays(container)).toEqual(['control-overlay']);
    expect(getByTestId('rail-control').getAttribute('aria-current')).toBe('page');
    expect(getByTestId('control-tab-chat').getAttribute('aria-selected')).toBe('true');
    expect(getByTestId('control-agent')).toBeTruthy();
    expect(queryByTestId('agent-panel')).toBeNull();
    await fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(get(destination)).toBe('session');
    expect(terminalSlot(container)).toBe(term);
  });

  it('⌘E opens and closes Control; ⌘⇧T opens its Today tab', async () => {
    const { container, getByTestId } = render(App);
    await fireEvent.keyDown(window, { key: 'e', metaKey: true });
    expect(get(destination)).toBe('control');
    expect(get(controlTab)).toBe('chat');
    await fireEvent.keyDown(window, { key: 'T', ctrlKey: true, shiftKey: true });
    expect(get(controlTab)).toBe('today');
    await waitFor(() => expect(getByTestId('control-tab-today').getAttribute('aria-selected')).toBe('true'));
    await fireEvent.keyDown(window, { key: 'T', ctrlKey: true, shiftKey: true });
    expect(get(destination)).toBe('session');
    await fireEvent.keyDown(window, { key: 'e', metaKey: true });
    await fireEvent.keyDown(window, { key: 'e', metaKey: true });
    expect(get(destination)).toBe('session');
    expect(openOverlays(container)).toEqual([]);
  });

});
