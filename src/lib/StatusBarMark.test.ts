// Redesign step 3.14: the status bar's 16 px mark, one state each.
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach } from 'vitest';
import { tick } from 'svelte';
import StatusBarMark from './StatusBarMark.svelte';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { sessions, sessionsAnswered } from './sessions';
import { host, session } from './hosts_fixture';
import { hosts } from './hosts';
import { catchingUp, resetStartup, warmStart } from './startup';

const REMOTE = { ...STANDALONE, remote: true, url: 'https://fleet.example.com' };
const mark = () => screen.queryByTestId('status-mark');

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  sessions.set([session('trn', 'dev-a')]);
  sessionsAnswered.set(true);
  hosts.set([]);
  catchingUp.set(new Set());
  resetStartup();
});

describe('StatusBarMark', () => {
  it('breathes while idle and connected, standalone included', () => {
    render(StatusBarMark);
    expect(mark()?.dataset.mark).toBe('breathe');
    expect(screen.getByTestId('status-mark-loader')).toHaveAttribute('aria-label', 'Idle and connected');
  });

  it('turns to Halo with the count when sessions wait for you, and back', async () => {
    render(StatusBarMark);
    sessions.set([session('trn', 'a', { claude_status: 'blocked' }), session('nas', 'b', { claude_status: 'blocked' })]);
    await tick();
    expect(mark()?.dataset.mark).toBe('halo');
    expect(mark()).toHaveAttribute('title', '2 sessions wait for you');
    sessions.set([session('trn', 'a')]);
    await tick();
    expect(mark()?.dataset.mark).toBe('breathe');
  });

  it('chases while a hub client connects, and breathes once connected', async () => {
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connecting' });
    render(StatusBarMark);
    expect(mark()?.dataset.mark).toBe('chase');
    hubConnection.set({ state: 'connected' });
    await tick();
    expect(mark()?.dataset.mark).toBe('breathe');
  });

  it('leaves a lost link to the banner, and a skewed hub to its sentence', async () => {
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'reconnecting', attempt: 1, retry_in_secs: 1, reason: 'eof' });
    render(StatusBarMark);
    expect(mark()).toBeNull();
    hubConnection.set({ state: 'hub_too_old', hub_contract: 0, min_contract: 2 });
    await tick();
    expect(mark()).toBeNull();
  });

  it('shows Signal lost when the configured hub is unavailable', () => {
    hubStatus.set({ ...STANDALONE, unavailable: 'the token is missing' });
    render(StatusBarMark);
    expect(mark()?.dataset.mark).toBe('signal-lost');
  });

  it('stays out of the way while the fleet arrives', () => {
    sessionsAnswered.set(false);
    render(StatusBarMark);
    expect(mark()).toBeNull();
  });

  it('breathes while a warm start re-syncs, where a cold one has its splash (step 3.15)', () => {
    sessionsAnswered.set(false);
    warmStart.set(true);
    render(StatusBarMark);
    expect(mark()?.dataset.mark).toBe('breathe');
    expect(mark()).toHaveAttribute('title', 'Re-syncing');
  });

  it('says a host that had not answered is still connecting, until it does', async () => {
    hosts.set([host('mac'), host('trn', { reachable: false })]);
    catchingUp.set(new Set(['trn']));
    render(StatusBarMark);
    expect(screen.getByTestId('status-catch-up').textContent).toBe('trn still connecting');
    hosts.set([host('mac'), host('trn', { reachable: true })]);
    await tick();
    expect(screen.queryByTestId('status-catch-up')).toBeNull();
  });
});
