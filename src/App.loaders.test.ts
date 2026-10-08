// Redesign step 3.13: loaders in the new shell. One loader per screen: while
// the first session list is out, the empty pane shows the Particle swarm and
// nothing else animates; once it answers, the pane asks for a pick and the
// only mark left is the status bar's idle Breathe (step 3.14), not a wait.
import { render, screen, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import App from './App.svelte';
import { onboardingDismissed, onboardingWelcomed } from './lib/onboarding';
import { clearToasts } from './lib/toasts';
import { clearSelection } from './lib/selection';
import { settingsOpen } from './lib/app_views';
import { sessionsAnswered, type SessionRow } from './lib/sessions';
import { fleetAccounts, fleetHosts, session } from './lib/hosts_fixture';
import { uiLayout } from './lib/prefs';
import { resetStartup } from './lib/startup';

let original: ((cmd: string, ...rest: unknown[]) => Promise<unknown>) | undefined;
let inv: ReturnType<typeof vi.fn>;
let answer: (rows: SessionRow[]) => void = () => {};

/** Loaders past their 400 ms delay, anywhere in the document. */
const shownLoaders = () => Array.from(document.querySelectorAll<HTMLElement>('[data-loader]'));

beforeEach(async () => {
  onboardingDismissed.set(true);
  onboardingWelcomed.set(true);
  clearToasts();
  clearSelection();
  settingsOpen.set(false);
  sessionsAnswered.set(false);
  localStorage.clear();
  resetStartup();
  uiLayout.set('classic');
  const { invoke } = await import('@tauri-apps/api/core');
  inv = invoke as ReturnType<typeof vi.fn>;
  original = inv.getMockImplementation() as typeof original;
  const pending = new Promise<SessionRow[]>((resolve) => (answer = resolve));
  inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
    switch (cmd) {
      case 'list_hosts':
        return fleetHosts();
      case 'list_accounts':
        return fleetAccounts();
      case 'list_sessions':
        // The first list is held until the test answers it; a later reload
        // (window focus) answers at once with the same rows.
        return pending;
      case 'list_projects':
      case 'list_account_usage':
      case 'list_host_tokens':
      case 'tunnel_status':
        return [];
      case 'check_local_prereqs':
        return { tmux: true, claude: true, git: true, ssh: true };
      default:
        return original ? original(cmd, ...rest) : null;
    }
  });
});

afterEach(() => {
  inv.mockImplementation(original!);
  clearSelection();
  settingsOpen.set(false);
});

describe('App: loaders in the new shell (redesign 3.13)', () => {
  it('shows the Particle swarm, and only it, while the fleet arrives', async () => {
    render(App);
    const swarm = await screen.findByTestId('fleet-arriving', {}, { timeout: 2000 });
    expect(swarm.dataset.loader).toBe('particle-swarm');
    expect(screen.getByText('Hosts and sessions arriving…')).toBeInTheDocument();
    expect(shownLoaders().map((l) => l.dataset.loader)).toEqual(['particle-swarm']);
  });

  it('once the first list answers, leaves only the status bar’s idle Breathe (3.14)', async () => {
    render(App);
    await screen.findByTestId('fleet-arriving', {}, { timeout: 2000 });
    answer([session('mefistos', 'dev-mef', { project_id: null })]);
    await waitFor(() => expect(screen.queryByTestId('fleet-arriving')).toBeNull());
    expect(screen.getByText('Select a session to attach a terminal.')).toBeInTheDocument();
    await waitFor(() => expect(shownLoaders().map((l) => l.dataset.loader)).toEqual(['breathe']));
  });
});

describe('App: startup in the new shell (redesign 3.15)', () => {
  it('a cold start shows the splash as the one loader, then leaves the app loaded under it', async () => {
    uiLayout.set('new');
    render(App);
    const splash = await screen.findByTestId('startup-splash', {}, { timeout: 2000 });
    await waitFor(() => expect(splash.dataset.stage).toBe('sessions'));
    await waitFor(() => expect(shownLoaders().map((l) => l.dataset.loader)).toEqual(['assemble']));
    expect(screen.queryByTestId('fleet-arriving')).toBeNull();
    answer([session('mefistos', 'dev-mef', { project_id: null })]);
    await waitFor(() => expect(screen.queryByTestId('startup-splash')).toBeNull());
    await waitFor(() => expect(shownLoaders().map((l) => l.dataset.loader)).toEqual(['breathe']));
  });

  it('a warm start shows no splash', async () => {
    localStorage.setItem('cf:startup:last-active', String(Date.now() - 60_000));
    uiLayout.set('new');
    render(App);
    await screen.findByTestId('fleet-arriving', {}, { timeout: 2000 });
    expect(screen.queryByTestId('startup-splash')).toBeNull();
  });
});
