// Redesign step 3.15: the startup stages, warm start, the update reveal and
// the hosts still catching up.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';
import {
  catchingUp,
  catchUpLine,
  hostsStillConnecting,
  isWarm,
  markStartup,
  releaseNotesUrl,
  resetStartup,
  startCatchUp,
  startupFacts,
  startupStage,
  takeUpdateReveal,
  trackActivity,
  updateToReveal,
  warmStart,
  WARM_START_MS,
  type StartupFacts,
} from './startup';
import { host } from './hosts_fixture';
import { destination, goTo, lastDestination, rereadLastDestination } from './destination';

const F = (over: Partial<StartupFacts> = {}): StartupFacts => ({
  backend: false,
  hosts: false,
  sessions: false,
  done: false,
  ...over,
});

beforeEach(() => {
  resetStartup();
  localStorage.clear();
});
afterEach(() => vi.useRealTimers());

describe('startupStage', () => {
  const standalone = { state: 'standalone' } as const;

  it('walks store, hosts, sessions, done for a standalone desktop (no hub stage)', () => {
    expect(startupStage(F(), false, standalone)).toBe('store');
    expect(startupStage(F({ backend: true }), false, standalone)).toBe('hosts');
    expect(startupStage(F({ backend: true, hosts: true }), false, standalone)).toBe('sessions');
    expect(startupStage(F({ backend: true, hosts: true, sessions: true }), false, standalone)).toBe('done');
  });

  it('waits on the hub link for a hub client', () => {
    expect(startupStage(F({ backend: true }), true, { state: 'connecting' })).toBe('hub');
    expect(startupStage(F({ backend: true, hosts: true }), true, { state: 'offline', attempt: 2, retry_in_secs: 4, reason: 'refused' })).toBe('hub');
    expect(startupStage(F({ backend: true }), true, { state: 'connected' })).toBe('hosts');
  });

  it('ends whenever every load has answered, a failed one included', () => {
    expect(startupStage(F({ done: true }), true, { state: 'connecting' })).toBe('done');
  });

  it('marks are one-way', () => {
    markStartup('backend');
    markStartup('backend');
    expect(get(startupFacts)).toEqual(F({ backend: true }));
  });
});

describe('warm start', () => {
  const NOW = 1_800_000_000_000;

  it('is warm within 8 h of last use, cold after or with no stamp', () => {
    expect(isWarm(null, NOW)).toBe(false);
    expect(isWarm(NOW - 60_000, NOW)).toBe(true);
    expect(isWarm(NOW - WARM_START_MS + 1, NOW)).toBe(true);
    expect(isWarm(NOW - WARM_START_MS, NOW)).toBe(false);
    // A clock that went backwards is not a reason to skip the splash.
    expect(isWarm(NOW + 60_000, NOW)).toBe(false);
  });

  it('reads the stamp the last run left, then keeps it fresh', () => {
    let t = NOW;
    localStorage.setItem('cf:startup:last-active', String(NOW - 3_600_000));
    const stop = trackActivity(() => t);
    expect(get(warmStart)).toBe(true);
    expect(localStorage.getItem('cf:startup:last-active')).toBe(String(NOW));
    t = NOW + 5000;
    window.dispatchEvent(new Event('pagehide'));
    expect(localStorage.getItem('cf:startup:last-active')).toBe(String(NOW + 5000));
    stop();
  });

  it('a first launch is cold', () => {
    const stop = trackActivity(() => NOW);
    expect(get(warmStart)).toBe(false);
    stop();
  });

  // The board's warm start: the last screen at once, no splash.
  it('a warm start puts back the screen the last run left', () => {
    goTo('hosts');
    expect(localStorage.getItem('cf:pref:nav.destination')).toBe(JSON.stringify('hosts'));
    // A new run: the stored value is read before App sets the Session tab.
    rereadLastDestination();
    destination.set('session');
    expect(lastDestination()).toBe('hosts');
    localStorage.setItem('cf:startup:last-active', String(NOW - 60_000));
    const stop = trackActivity(() => NOW);
    expect(get(warmStart)).toBe(true);
    expect(get(destination)).toBe('hosts');
    stop();
  });

  it('a cold start opens on the Session tab, whatever the last run left', () => {
    goTo('assets');
    rereadLastDestination();
    destination.set('session');
    localStorage.setItem('cf:startup:last-active', String(NOW - WARM_START_MS - 1));
    const stop = trackActivity(() => NOW);
    expect(get(warmStart)).toBe(false);
    expect(get(destination)).toBe('session');
    stop();
  });

  it('a stored value that is no destination is ignored', () => {
    localStorage.setItem('cf:pref:nav.destination', JSON.stringify('nowhere'));
    rereadLastDestination();
    expect(lastDestination()).toBe('session');
  });
});

describe('update reveal', () => {
  it('plays once for a new version, never on the first launch that knows the rule', () => {
    expect(updateToReveal(null, '0.5.4')).toBeNull();
    expect(updateToReveal('0.5.4', '0.5.4')).toBeNull();
    expect(updateToReveal('0.5.3', '0.5.4')).toBe('0.5.4');
    expect(updateToReveal('0.5.3', null)).toBeNull();
  });

  it('records the version it saw', () => {
    expect(takeUpdateReveal('0.5.3')).toBeNull();
    expect(takeUpdateReveal('0.5.4')).toBe('0.5.4');
    expect(takeUpdateReveal('0.5.4')).toBeNull();
  });

  it("links What's new to the release", () => {
    expect(releaseNotesUrl('0.5.4')).toBe('https://github.com/martin-janci/claude-fleet/releases/tag/v0.5.4');
  });
});

describe('hosts still connecting', () => {
  it('names the visible hosts that had not answered, until they do', () => {
    const list = [host('mac'), host('trn', { reachable: false }), host('old', { reachable: false, hidden: true })];
    const stop = startCatchUp(list);
    expect([...get(catchingUp)]).toEqual(['trn']);
    expect(hostsStillConnecting(list, get(catchingUp))).toEqual(['trn']);
    expect(hostsStillConnecting([host('mac'), host('trn')], get(catchingUp))).toEqual([]);
    stop();
  });

  it('stops saying so after 20 s, so a host that is really down is not "connecting"', () => {
    vi.useFakeTimers();
    startCatchUp([host('trn', { reachable: false })], 20_000);
    vi.advanceTimersByTime(19_999);
    expect(get(catchingUp).size).toBe(1);
    vi.advanceTimersByTime(1);
    expect(get(catchingUp).size).toBe(0);
  });

  it('in words', () => {
    expect(catchUpLine([])).toBeNull();
    expect(catchUpLine(['claude-fleet-trn'])).toBe('claude-fleet-trn still connecting');
    expect(catchUpLine(['a', 'b', 'c'])).toBe('a and 2 more still connecting');
  });
});
