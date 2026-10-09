// Redesign step 3.15: startup as on the Startup board. Each stage shows the
// loader for what is really happening, and a stage that ends inside the
// 400 ms loader delay shows none of its own.
//
//   store     Draw-on      the backend's first answer (hub status, health,
//                          the row subscription). The database itself is
//                          opened and migrated in the backend's setup
//                          closure (src-tauri lib.rs), which Tauri runs on
//                          the main thread after building this window but
//                          before its event loop starts: the page cannot
//                          load, paint or receive an event until the
//                          migration is over, so there is no migration to
//                          show a Progress ring for. Showing one would mean
//                          opening the store after the window, behind every
//                          command that takes it as managed state.
//   hub       Chase        a hub client whose link is not up yet; standalone
//                          skips it. After 6 s it reads Signal lost, with
//                          Open offline (offline.ts), Retry and Hub settings.
//   hosts     Radar        the host list, one blip per host that answers
//   sessions  Assemble     the session list ("22 sessions · 4 need you")
//   done                   the splash shrinks away; a host that has not
//                          answered keeps "still connecting" in the status bar
//
// Warm start (opened again within 8 h of last being used) shows no splash at
// all: the last screen at once (the stored destination, destination.ts),
// and Breathe in the status bar while the list re-syncs. After an update, the Wordmark reveal plays once with the new
// version and a link to what changed.
import { writable } from 'svelte/store';
import type { HubConnection } from './hub_connection';
import type { HostRow } from './hosts';
import { restoreLastDestination } from './destination';

export type StartupStage = 'store' | 'hub' | 'hosts' | 'sessions' | 'done';

/** What has answered so far; App.svelte marks each as it lands. */
export interface StartupFacts {
  backend: boolean;
  hosts: boolean;
  sessions: boolean;
  /** Every startup load has answered (or failed): nothing is waited on. */
  done: boolean;
}

const NONE: StartupFacts = { backend: false, hosts: false, sessions: false, done: false };

export const startupFacts = writable<StartupFacts>({ ...NONE });

export function markStartup(fact: keyof StartupFacts): void {
  startupFacts.update((f) => (f[fact] ? f : { ...f, [fact]: true }));
}

/** For tests: back to a fresh launch. */
export function resetStartup(): void {
  startupFacts.set({ ...NONE });
  warmStart.set(false);
  splashShown.set(false);
}

/** The stage on screen, from what has answered. `remote`: a hub client. */
export function startupStage(f: StartupFacts, remote: boolean, conn: HubConnection): StartupStage {
  if (f.done) return 'done';
  if (!f.backend) return 'store';
  if (remote && conn.state !== 'connected') return 'hub';
  if (!f.hosts) return 'hosts';
  if (!f.sessions) return 'sessions';
  return 'done';
}

/** The splash steps aside after this long, whatever is still loading. */
export const GIVE_UP_MS = 20_000;

/** The splash is on screen: it is that screen's one loader, so the empty
 *  pane under it holds its Particle swarm back (StartupSplash sets it). */
export const splashShown = writable<boolean>(false);

/** The board's warm start: opened again within 8 h. */
export const WARM_START_MS = 8 * 60 * 60 * 1000;

const LAST_ACTIVE_KEY = 'cf:startup:last-active';
const SEEN_VERSION_KEY = 'cf:startup:seen-version';

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, v: string): void {
  try {
    localStorage.setItem(key, v);
  } catch {
    // A private window or blocked storage: the next launch is a cold one.
  }
}

/** Whether a launch at `now` is warm, given when the app was last in use. */
export function isWarm(lastActiveAt: number | null, now: number): boolean {
  return lastActiveAt !== null && now >= lastActiveAt && now - lastActiveAt < WARM_START_MS;
}

/** Set once at launch from the stored stamp; read by the splash and the
 *  status bar's mark. */
export const warmStart = writable<boolean>(false);

/** Read the stamp the last run left, decide warm or cold (warm puts the last
 *  destination back), and keep the stamp fresh from now on: on every hide and once a minute while open. Returns
 *  the cleanup. */
export function trackActivity(now: () => number = Date.now): () => void {
  const raw = read(LAST_ACTIVE_KEY);
  const last = raw === null ? null : Number(raw);
  const warm = isWarm(Number.isFinite(last) ? last : null, now());
  warmStart.set(warm);
  // The last screen at once: the session is restored by its own pref
  // (`session.last`), the destination over it here.
  if (warm) restoreLastDestination();
  const stamp = () => write(LAST_ACTIVE_KEY, String(now()));
  stamp();
  const t = setInterval(stamp, 60_000);
  const onHide = () => {
    if (document.hidden) stamp();
  };
  document.addEventListener('visibilitychange', onHide);
  window.addEventListener('pagehide', stamp);
  return () => {
    clearInterval(t);
    document.removeEventListener('visibilitychange', onHide);
    window.removeEventListener('pagehide', stamp);
  };
}

/** The version to reveal once after an update, or null. The first launch
 *  that knows this rule only records the version: it cannot tell an update
 *  from a fresh install. */
export function updateToReveal(seen: string | null, current: string | null): string | null {
  if (current === null || seen === null || seen === current) return null;
  return current;
}

/** Compare with the version the last launch saw, and record this one. */
export function takeUpdateReveal(current: string | null): string | null {
  if (current === null) return null;
  const v = updateToReveal(read(SEEN_VERSION_KEY), current);
  write(SEEN_VERSION_KEY, current);
  return v;
}

/** Where "What's new" goes: the release's notes. */
export function releaseNotesUrl(version: string): string {
  return `https://github.com/martin-janci/claude-fleet/releases/tag/v${version}`;
}

/** How long the status bar keeps saying a host is still connecting. */
export const HOST_CATCH_UP_MS = 20_000;

/** Hosts the status bar names as still connecting once the app is in: the
 *  visible ones that had not answered when the list came, until they do. */
export function hostsStillConnecting(list: readonly HostRow[], waiting: ReadonlySet<string>): string[] {
  return list.filter((h) => !h.hidden && !h.reachable && waiting.has(h.alias)).map((h) => h.alias);
}

/** Aliases that had not answered when startup ended; emptied after
 *  HOST_CATCH_UP_MS so a host that is really down stops "connecting". */
export const catchingUp = writable<ReadonlySet<string>>(new Set());

export function startCatchUp(list: readonly HostRow[], ms = HOST_CATCH_UP_MS): () => void {
  catchingUp.set(new Set(list.filter((h) => !h.hidden && !h.reachable).map((h) => h.alias)));
  const t = setTimeout(() => catchingUp.set(new Set()), ms);
  return () => clearTimeout(t);
}

/** "mercury still connecting", "mercury and 2 more still connecting". */
export function catchUpLine(aliases: readonly string[]): string | null {
  if (aliases.length === 0) return null;
  const rest = aliases.length - 1;
  return `${aliases[0]}${rest > 0 ? ` and ${rest} more` : ''} still connecting`;
}
