// Redesign step 3.5: the session header and one tab bar, and the inspector
// (0.5.x's Details pane, moved beside the session) on ⌥⌘B / Ctrl+Alt+B.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, beforeAll, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';
import App from './App.svelte';
import { onboardingDismissed, onboardingWelcomed } from './lib/onboarding';
import { clearToasts } from './lib/toasts';
import { clearSelection, selectSession } from './lib/selection';
import { hostFilter } from './lib/hosts';
import { settingsOpen } from './lib/app_views';
import { destination } from './lib/destination';
import { sessionActionRequest } from './lib/session_actions';
import { detectMac } from './lib/terminal_keys';
import type { SessionRow } from './lib/sessions';
import { session } from './lib/hosts_fixture';
import { preloadLazyViews } from './lib/lazy_views';

// The off-screen views load lazily in the app; here they are in place
// before the first render, so a test sees them on the frame they open.
beforeAll(() => preloadLazyViews());

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
  it('no session: a quiet empty state, and no session header or tabs (UX audit N1, N4)', async () => {
    await mountApp();
    expect(screen.getByTestId('details-view')).toBeTruthy();
    expect(screen.getByTestId('no-session')).toBeTruthy();
    expect(screen.queryByText('Morning brief')).toBeNull();
    expect(screen.queryByTestId('inspector')).toBeNull();
    expect(screen.queryByTestId('session-head')).toBeNull();
    expect(screen.queryByTestId('stab-conversation')).toBeNull();
  });

  it('a selected session gets its header, the agent tab and the inspector beside it', async () => {
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

  it('Open in VS Code: the header button and its chord open the selected session (step 5.5)', async () => {
    await mountApp();
    selectSession(rows[0]);
    const btn = (await screen.findByTestId('open-in-editor')) as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
    const args = { args: { host_alias: 'mefistos', tmux_name: 'dev-mef' } };
    await fireEvent.click(btn);
    await waitFor(() => expect(inv).toHaveBeenCalledWith('open_session_in_editor', args));
    inv.mockClear();
    await fireEvent.keyDown(document.body, isMac
      ? { key: 'e', code: 'KeyE', metaKey: true, shiftKey: true }
      : { key: 'e', code: 'KeyE', ctrlKey: true, altKey: true });
    await waitFor(() => expect(inv).toHaveBeenCalledWith('open_session_in_editor', args));
  });

  it('the inspector chord and the header button toggle it, and it is remembered', async () => {
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

  it('a row action opens the inspector it runs in', async () => {
    await mountApp();
    selectSession(rows[0]);
    await screen.findByTestId('inspector');
    await inspectorChord();
    await waitFor(() => expect(screen.queryByTestId('inspector')).toBeNull());
    sessionActionRequest.set({ sessionId: rows[0].id, action: 'details', seq: 1 });
    await waitFor(() => expect(screen.getByTestId('inspector')).toBeTruthy());
    sessionActionRequest.set(null);
  });
});
