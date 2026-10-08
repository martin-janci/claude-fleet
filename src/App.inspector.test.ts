// Redesign step 3.5: the New layout's session header and one tab bar, and
// the inspector (Classic's Details pane, moved beside the session) on
// ⌥⌘B / Ctrl+Alt+B. Classic keeps its tabs and its center pane.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';
import App from './App.svelte';
import { onboardingDismissed, onboardingWelcomed } from './lib/onboarding';
import { clearToasts } from './lib/toasts';
import { clearSelection, selectSession } from './lib/selection';
import { hostFilter } from './lib/hosts';
import { settingsOpen } from './lib/app_views';
import { destination } from './lib/destination';
import { uiLayout } from './lib/prefs';
import { detectMac } from './lib/terminal_keys';
import type { SessionRow } from './lib/sessions';
import { session } from './lib/hosts_fixture';

let rows: SessionRow[] = [];
let original: ((cmd: string, ...rest: unknown[]) => Promise<unknown>) | undefined;
let inv: ReturnType<typeof vi.fn>;

beforeEach(async () => {
  onboardingDismissed.set(true);
  onboardingWelcomed.set(true);
  clearToasts();
  clearSelection();
  hostFilter.set('all');
  settingsOpen.set(false);
  localStorage.clear();
  rows = [session('mefistos', 'dev-mef', { project_id: null })];
  const { invoke } = await import('@tauri-apps/api/core');
  inv = invoke as ReturnType<typeof vi.fn>;
  original = inv.getMockImplementation() as typeof original;
  inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
    switch (cmd) {
      case 'list_sessions':
        return rows;
      case 'list_hosts':
      case 'list_accounts':
      case 'list_projects':
      case 'list_account_usage':
      case 'list_host_tokens':
      case 'tunnel_status':
      case 'repo_changes':
        return [];
      default:
        return original ? original(cmd, ...rest) : null;
    }
  });
});

afterEach(() => {
  inv.mockImplementation(original!);
  clearSelection();
  destination.set('session');
  uiLayout.set('classic');
});

async function mountApp() {
  const r = render(App);
  await waitFor(() => expect(screen.getAllByTestId('sess-row').length).toBe(1));
  return r;
}

const isMac = detectMac(navigator);
const inspectorChord = () =>
  fireEvent.keyDown(document.body, isMac
    ? { key: 'b', code: 'KeyB', metaKey: true, altKey: true }
    : { key: 'b', code: 'KeyB', ctrlKey: true, altKey: true });

describe('App: session tabs and the inspector (step 3.5)', () => {
  it('New layout: no session fills the right column with Details, and the tabs wait', async () => {
    uiLayout.set('new');
    await mountApp();
    expect(screen.getByTestId('details-view')).toBeTruthy();
    expect(screen.queryByTestId('inspector')).toBeNull();
    expect(screen.queryByTestId('center-collapse')).toBeNull();
    expect(screen.queryByTestId('tab-session')).toBeNull();
    expect((screen.getByTestId('stab-conversation') as HTMLButtonElement).disabled).toBe(true);
  });

  it('a selected session gets its header, the agent tab and the inspector beside it', async () => {
    uiLayout.set('new');
    await mountApp();
    selectSession(rows[0]);
    await waitFor(() => expect(screen.getByTestId('inspector')).toBeTruthy());
    expect(screen.queryByTestId('details-view')).toBeNull();
    expect(screen.getByTestId('session-head-name').textContent).toBe('dev-mef');
    expect(screen.getByTestId('session-head-meta').textContent).toContain('mefistos');
    expect(screen.getByTestId('stab-agent').getAttribute('aria-selected')).toBe('true');
    // The pane's host is in the header, so Hosts leaves the bar (rail, ⌘I).
    expect(screen.queryByTestId('tab-hosts')).toBeNull();
  });

  it('the Details tab takes the column and the inspector steps aside', async () => {
    uiLayout.set('new');
    await mountApp();
    selectSession(rows[0]);
    await screen.findByTestId('inspector');
    await fireEvent.click(screen.getByTestId('stab-details'));
    expect(get(destination)).toBe('details');
    expect(screen.getByTestId('details-view')).toBeTruthy();
    expect(screen.queryByTestId('inspector')).toBeNull();
    expect((screen.getByTestId('inspector-toggle') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(screen.getByTestId('stab-agent'));
    expect(get(destination)).toBe('session');
    expect(screen.getByTestId('inspector')).toBeTruthy();
  });

  it('the inspector chord and the header button toggle it, and it is remembered', async () => {
    uiLayout.set('new');
    await mountApp();
    selectSession(rows[0]);
    await screen.findByTestId('inspector');
    await inspectorChord();
    await waitFor(() => expect(screen.queryByTestId('inspector')).toBeNull());
    expect(localStorage.getItem('cf:pref:layout.inspector')).toBe('false');
    expect(screen.getByTestId('inspector-toggle').getAttribute('aria-pressed')).toBe('false');
    await fireEvent.click(screen.getByTestId('inspector-toggle'));
    expect(screen.getByTestId('inspector')).toBeTruthy();
    expect(localStorage.getItem('cf:pref:layout.inspector')).toBe('true');
  });

  it('Classic keeps its tabs and its center Details pane; the chord folds the pane', async () => {
    await mountApp();
    selectSession(rows[0]);
    await screen.findByTestId('tab-session');
    expect(screen.queryByTestId('session-tabs')).toBeNull();
    expect(screen.queryByTestId('inspector')).toBeNull();
    expect(screen.getByTestId('center-collapse')).toBeTruthy();
    await inspectorChord();
    await waitFor(() => expect(screen.queryByTestId('center-expand')).not.toBeNull());
  });
});
