// Redesign step 3.13: ⌘K shows what it already holds at once, and while the
// first session list is still out it says which hosts it is still hearing
// from, with the kit's Dot wave.
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach } from 'vitest';
import { tick } from 'svelte';
import QuickSwitcher from './QuickSwitcher.svelte';
import { sessions, sessionsAnswered } from './sessions';
import { hosts } from './hosts';
import { projects } from './projects';
import { clearSelection } from './selection';
import { recentSessions } from './quick_switcher';
import { fleetHosts, host, session } from './hosts_fixture';

async function openSwitcher() {
  await fireEvent.keyDown(window, { key: 'K', ctrlKey: true, shiftKey: true });
  await tick();
}

beforeEach(() => {
  localStorage.clear();
  recentSessions.set([]);
  clearSelection();
  projects.set([]);
  sessions.set([session('mefistos', 'dev-mef', { project_id: null })]);
  hosts.set(fleetHosts());
  sessionsAnswered.set(false);
});

describe('QuickSwitcher while the fleet arrives (redesign 3.13)', () => {
  it('lists what it holds at once and names the hosts it is still hearing from', async () => {
    render(QuickSwitcher);
    await openSwitcher();
    expect(within(screen.getByTestId('switcher-list')).getByText(/dev-mef/)).toBeInTheDocument();
    const line = screen.getByTestId('switcher-still-hearing');
    expect(line).toHaveTextContent('Still hearing from local, mefistos, claude-fleet-htz and 2 more');
    // The Dot wave keeps its box, then appears after the kit's 400 ms.
    await waitFor(() => expect(within(line).getByTestId('loader').dataset.loader).toBe('dot-wave'));
  });

  it('leaves hidden hosts out, and goes once the list answers', async () => {
    hosts.set([host('trn'), host('nas', { hidden: true })]);
    render(QuickSwitcher);
    await openSwitcher();
    expect(screen.getByTestId('switcher-still-hearing')).toHaveTextContent('Still hearing from trn');
    sessionsAnswered.set(true);
    await tick();
    expect(screen.queryByTestId('switcher-still-hearing')).toBeNull();
  });

  it('with no hosts yet says the sessions are still arriving', async () => {
    hosts.set([]);
    render(QuickSwitcher);
    await openSwitcher();
    expect(screen.getByTestId('switcher-still-hearing')).toHaveTextContent('Sessions still arriving');
  });
});
