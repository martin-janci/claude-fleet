// Redesign step 3.1: one destination store owns the right column. Classic
// shows exactly what it showed with the old overlay flags, never two
// overlays at once, and the terminal underneath stays mounted through every
// overlay round trip.
import { render, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import App from './App.svelte';
import { onboardingDismissed } from './lib/onboarding';
import { clearToasts } from './lib/toasts';
import { workBoardOpen, requestHostsView, settingsOpen } from './lib/app_views';
import { sidebarView } from './lib/work_view';
import { destination } from './lib/destination';
import { uiLayout } from './lib/prefs';
import { sessionActionRequest } from './lib/session_actions';

const OVERLAYS = ['hosts-overlay', 'assets-overlay', 'board-overlay', 'accounts-overlay'];

beforeEach(() => {
  onboardingDismissed.set(true);
  clearToasts();
});
afterEach(() => {
  destination.set('session');
  uiLayout.set('classic');
  sidebarView.set('sessions');
  settingsOpen.set(false);
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
    const { getByTestId } = render(App);
    expect(get(destination)).toBe('session');
    expect(getByTestId('tab-session').classList.contains('active')).toBe(true);
  });

  it('keeps the terminal mounted, and one overlay at most, through every overlay', async () => {
    const { container, getByTestId } = render(App);
    const term = terminalSlot(container);

    await fireEvent.click(getByTestId('tab-assets'));
    expect(openOverlays(container)).toEqual(['assets-overlay']);

    await fireEvent.click(getByTestId('tab-hosts'));
    expect(get(destination)).toBe('hosts');
    expect(openOverlays(container)).toEqual(['hosts-overlay']);

    workBoardOpen.set(true);
    await waitFor(() => expect(openOverlays(container)).toEqual(['board-overlay']));
    expect(getByTestId('tab-session').classList.contains('active')).toBe(false);

    await fireEvent.click(getByTestId('tab-session'));
    expect(get(destination)).toBe('session');
    expect(openOverlays(container)).toEqual([]);
    expect(terminalSlot(container)).toBe(term);
  });

  it('Esc on the board returns to the Session tab', async () => {
    const { container } = render(App);
    workBoardOpen.set(true);
    await waitFor(() => expect(openOverlays(container)).toEqual(['board-overlay']));
    await fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(get(destination)).toBe('session');
    expect(get(workBoardOpen)).toBe(false);
  });

  it('a Hosts request from outside App replaces the open overlay', async () => {
    const { container, getByTestId } = render(App);
    await fireEvent.click(getByTestId('tab-assets'));
    requestHostsView();
    await waitFor(() => expect(openOverlays(container)).toEqual(['hosts-overlay']));
    expect(get(destination)).toBe('hosts');
  });

  it('New layout: the board is a Work view, with no close and no Esc', async () => {
    uiLayout.set('new');
    try {
      const { container, queryByTestId } = render(App);
      workBoardOpen.set(true);
      await waitFor(() => expect(queryByTestId('board-view')).not.toBeNull());
      expect(queryByTestId('board-overlay')).toBeNull();
      expect(queryByTestId('work-board-close')).toBeNull();
      await fireEvent.keyDown(document.body, { key: 'Escape' });
      expect(get(destination)).toBe('board');
      // The terminal stays mounted under it, as under every destination.
      expect(container.querySelector('.right-body > .view-slot:not(.overlay)')).not.toBeNull();
    } finally {
      uiLayout.set('classic');
    }
  });

  it('a row action shows the Details pane it runs in', async () => {
    const { getByTestId, queryByTestId } = render(App);
    await fireEvent.click(getByTestId('center-collapse'));
    expect(queryByTestId('center-expand')).not.toBeNull();
    sessionActionRequest.set({ sessionId: 1, action: 'details', seq: 1 });
    await waitFor(() => expect(queryByTestId('center-expand')).toBeNull());
    sessionActionRequest.set(null);
  });
});

describe('App: the rail and the Accounts page (steps 3.2, 4.1)', () => {
  it('Classic shows no rail', () => {
    const { queryByTestId } = render(App);
    expect(queryByTestId('rail')).toBeNull();
  });

  it('New shows the landed items in the manual order, Settings last', () => {
    uiLayout.set('new');
    const { getByTestId } = render(App);
    const ids = Array.from(getByTestId('rail').querySelectorAll('[data-testid^="rail-"]'), (e) =>
      e.getAttribute('data-testid'),
    );
    expect(ids).toEqual(['rail-sessions', 'rail-work', 'rail-accounts', 'rail-settings']);
    expect(getByTestId('rail-sessions').getAttribute('aria-current')).toBe('page');
  });

  it('Accounts opens as one more overlay over a mounted terminal, and Esc leaves it', async () => {
    uiLayout.set('new');
    const { container, getByTestId } = render(App);
    const term = terminalSlot(container);
    await fireEvent.click(getByTestId('tab-hosts'));
    expect(getByTestId('rail-accounts').getAttribute('aria-current')).toBe('page');
    await fireEvent.click(getByTestId('rail-accounts'));
    expect(openOverlays(container)).toEqual(['accounts-overlay']);
    expect(getByTestId('rail-accounts').getAttribute('aria-current')).toBe('page');
    expect(getByTestId('tab-session').classList.contains('active')).toBe(false);
    await fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(get(destination)).toBe('session');
    expect(terminalSlot(container)).toBe(term);
  });

  it('Work and Sessions pick the sidebar tree and leave a fleet page', async () => {
    uiLayout.set('new');
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

  it('Settings opens the Settings dialog', async () => {
    uiLayout.set('new');
    const { getByTestId } = render(App);
    await fireEvent.click(getByTestId('rail-settings'));
    expect(get(settingsOpen)).toBe(true);
    settingsOpen.set(false);
  });

  it('switching back to Classic leaves Accounts and hides the rail', async () => {
    uiLayout.set('new');
    const { container, getByTestId, queryByTestId } = render(App);
    await fireEvent.click(getByTestId('rail-accounts'));
    uiLayout.set('classic');
    await waitFor(() => expect(openOverlays(container)).toEqual([]));
    expect(queryByTestId('rail')).toBeNull();
    expect(get(destination)).toBe('session');
  });
});
