// Redesign step 3.15: the cold-start splash, one stage and one loader at a
// time, as on the Startup board.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(() => Promise.resolve(null)) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { invoke } from '@tauri-apps/api/core';
import StartupSplash from './StartupSplash.svelte';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { hosts } from './hosts';
import { sessions } from './sessions';
import { host, session } from './hosts_fixture';
import { markStartup, resetStartup, warmStart } from './startup';
import { leaveOffline, offlineSessions } from './offline';

const REMOTE = { ...STANDALONE, remote: true, url: 'fleet.rlt.sk' };
const splash = () => screen.queryByTestId('startup-splash');
const loader = () => screen.getByTestId('startup-loader').dataset.loader;
// A stage's loader keeps its box empty for the first 400 ms of the stage.
const settle = () => vi.advanceTimersByTimeAsync(400);

beforeEach(() => {
  vi.useFakeTimers();
  resetStartup();
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  hosts.set([]);
  sessions.set([]);
});
afterEach(() => {
  vi.useRealTimers();
  leaveOffline();
  offlineSessions.set([]);
  vi.mocked(invoke).mockImplementation(() => Promise.resolve(null));
});

async function mount(onhubsettings = vi.fn()) {
  render(StartupSplash, { props: { onhubsettings } });
  await vi.advanceTimersByTimeAsync(400);
  return onhubsettings;
}

describe('StartupSplash', () => {
  it('shows nothing for the first 400 ms, so a quick launch never flashes it', async () => {
    render(StartupSplash, { props: { onhubsettings: vi.fn() } });
    await vi.advanceTimersByTimeAsync(399);
    expect(splash()).toBeNull();
    await vi.advanceTimersByTimeAsync(1);
    expect(splash()?.dataset.stage).toBe('store');
  });

  it('a launch that is in before 400 ms shows no splash at all', async () => {
    render(StartupSplash, { props: { onhubsettings: vi.fn() } });
    markStartup('done');
    await vi.advanceTimersByTimeAsync(400);
    expect(splash()).toBeNull();
  });

  it('walks Draw-on, Radar with the hosts that answered, Assemble with the count', async () => {
    await mount();
    await settle();
    expect(loader()).toBe('draw-on');
    hosts.set([host('mac'), host('mercury'), host('claude-fleet-trn', { reachable: false })]);
    markStartup('backend');
    await tick();
    expect(splash()?.dataset.stage).toBe('hosts');
    expect(screen.queryByTestId('startup-loader')).toBeNull();
    await settle();
    expect(loader()).toBe('radar');
    expect(screen.getByTestId('startup-detail').textContent).toBe('2 of 3 answered');
    expect(screen.getByTestId('startup-hosts').textContent).toContain('claude-fleet-trn …');
    // One blip per host that answered, and the next blip when the next does.
    const blips = () => screen.getByTestId('startup-loader').querySelectorAll('.ofl-rd2 i').length;
    expect(blips()).toBe(2);
    hosts.set([host('mac'), host('mercury'), host('claude-fleet-trn')]);
    await tick();
    expect(blips()).toBe(3);
    sessions.set([session('mac', 'a'), session('mac', 'b', { claude_status: 'blocked' })]);
    markStartup('hosts');
    await settle();
    expect(loader()).toBe('assemble');
    expect(screen.getByTestId('startup-detail').textContent).toBe('2 sessions · 1 need you');
    // One particle per session.
    expect(screen.getByTestId('startup-loader').querySelectorAll('.ofl-cv i')).toHaveLength(2);
    markStartup('sessions');
    markStartup('done');
    await vi.advanceTimersByTimeAsync(400);
    expect(splash()).toBeNull();
  });

  it('a standalone desktop skips the hub stage', async () => {
    await mount();
    markStartup('backend');
    await tick();
    expect(splash()?.dataset.stage).toBe('hosts');
  });

  it('a hub client chases the hub, then reads Signal lost after 6 s with Retry and Hub settings', async () => {
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connecting' });
    const onhubsettings = await mount();
    markStartup('backend');
    await settle();
    expect(loader()).toBe('chase');
    expect(screen.getByTestId('startup-detail').textContent).toBe('fleet.rlt.sk');
    await vi.advanceTimersByTimeAsync(6000);
    expect(splash()?.dataset.stage).toBe('hub-lost');
    expect(loader()).toBe('signal-lost');
    expect(screen.getByTestId('startup-title').textContent).toBe('Cannot reach fleet.rlt.sk');
    await fireEvent.click(screen.getByTestId('startup-retry'));
    expect(invoke).toHaveBeenCalledWith('hub_retry_now', undefined);
    await fireEvent.click(screen.getByTestId('startup-hub-settings'));
    expect(onhubsettings).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(300);
    expect(splash()).toBeNull();
  });

  // The plan's Verified by: Open offline lists only local hosts.
  it('Open offline lists only this computer, never the hub hosts it has seen', async () => {
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connecting' });
    // Hosts the window already holds from the hub, and one of its sessions.
    hosts.set([host('mercury'), host('claude-fleet-trn')]);
    sessions.set([session('mercury', 'hub-only')]);
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === 'offline_local_sessions'
          ? [
              { name: 'notes', created: 1, last_activity: 5, attached: false, attach: "tmux attach -t '=notes'" },
              { name: 'api', created: 1, last_activity: 9, attached: true, attach: "tmux attach -t '=api'" },
            ]
          : null,
      ) as never,
    );
    await mount();
    markStartup('backend');
    await settle();
    await vi.advanceTimersByTimeAsync(6000);
    await fireEvent.click(screen.getByTestId('startup-offline'));
    await vi.advanceTimersByTimeAsync(300);
    expect(splash()).toBeNull();
    expect(invoke).toHaveBeenCalledWith('offline_local_sessions', undefined);
    const view = screen.getByTestId('offline-fleet');
    const hostsShown = Array.from(view.querySelectorAll('[data-testid="offline-host"]')).map((h) => (h as HTMLElement).dataset.alias);
    expect(hostsShown).toEqual(['local']);
    expect(view.textContent).not.toContain('mercury');
    expect(view.textContent).not.toContain('claude-fleet-trn');
    expect(view.textContent).not.toContain('hub-only');
    // This computer's own sessions, most recent first.
    expect(screen.getAllByTestId('offline-session').map((r) => r.querySelector('.name')?.textContent)).toEqual(['api', 'notes']);
    await fireEvent.click(screen.getAllByTestId('offline-editor')[0]);
    expect(invoke).toHaveBeenCalledWith('open_session_in_editor', { args: { host_alias: 'local', tmux_name: 'api' } });
    // The hub answers: the offline view steps aside for the app.
    hubConnection.set({ state: 'connected' });
    await tick();
    expect(screen.queryByTestId('offline-fleet')).toBeNull();
  });

  it('a standalone desktop is never offered Open offline', async () => {
    await mount();
    markStartup('backend');
    await vi.advanceTimersByTimeAsync(7000);
    expect(screen.queryByTestId('startup-offline')).toBeNull();
  });

  it('moves on once the hub answers', async () => {
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connecting' });
    await mount();
    markStartup('backend');
    hubConnection.set({ state: 'connected' });
    await tick();
    expect(splash()?.dataset.stage).toBe('hosts');
  });

  it('never shows on a warm start', async () => {
    warmStart.set(true);
    await mount();
    expect(splash()).toBeNull();
  });

  it('leaves an unavailable hub to its banner', async () => {
    hubStatus.set({ ...STANDALONE, unavailable: 'no token' });
    await mount();
    expect(splash()).toBeNull();
  });

  it('gives the window back after 20 s rather than hide a stuck startup', async () => {
    await mount();
    expect(splash()).not.toBeNull();
    await vi.advanceTimersByTimeAsync(20_300);
    expect(splash()).toBeNull();
  });
});
