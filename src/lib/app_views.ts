// App-level view requests that cross component boundaries. App.svelte owns
// the Hosts mode (it shares the Files-mode overlay, so the terminal stays
// mounted) and the Sidebar owns Settings and the project picker; the quick
// switcher, Settings, the onboarding card and the Hosts view reach them
// through these stores instead of prop-drilling. Same pattern as
// `new_session_request.ts`.
import { writable } from 'svelte/store';

export interface HostsViewRequest {
  /** Host alias to preselect; `null` lets App pick (selected session's host,
   *  else the last-viewed host, else the view's own default). */
  host: string | null;
}

/** A pending "open the Hosts view" request; App consumes and clears it. */
export const hostsViewRequest = writable<HostsViewRequest | null>(null);

/** Whether the Hosts view is showing. Written by App only. */
export const hostsViewOpen = writable(false);

export function requestHostsView(host: string | null = null): void {
  hostsViewRequest.set({ host });
}

// "Close the Hosts view" — the flip side of `onSessionOpened` in
// selection.ts. `viewHostSessions` (host_actions.ts) fires this after
// jumping the sidebar filter to a host so a click deep in the Hosts overlay
// (HostDetail's "View sessions" button, the `s` key) can close the overlay
// without importing App.svelte. App is the only listener in practice,
// mirroring `closeHosts()` on `onSessionOpened`.
const hostsCloseListeners = new Set<() => void>();

/** Subscribe to "close the Hosts view" requests; returns the unsubscribe. */
export function onHostsCloseRequested(fn: () => void): () => void {
  hostsCloseListeners.add(fn);
  return () => hostsCloseListeners.delete(fn);
}

export function requestCloseHosts(): void {
  for (const fn of hostsCloseListeners) fn();
}

/** Settings dialog visibility (mounted by the Sidebar; ⌘, sets it). */
export const settingsOpen = writable(false);

/** A Settings section to scroll to when the dialog next opens (`work`:
 *  Settings → Work); the section clears it once shown. */
export const settingsFocus = writable<'work' | null>(null);

/** Open Settings at `section`. */
export function openSettingsAt(section: 'work'): void {
  settingsFocus.set(section);
  settingsOpen.set(true);
}

/**
 * "Start a new session on this host": the Sidebar opens its project picker
 * and preselects the host in the NewSessionDialog that follows.
 */
export const newSessionHostRequest = writable<string | null>(null);

export function requestNewSessionOnHost(host: string): void {
  newSessionHostRequest.set(host);
}

/**
 * "Open this file in the Files tab": a path clicked in the Conversation tab.
 * App switches to Files for the session; FilesPanel selects the path and
 * clears the request.
 */
export interface OpenPathRequest {
  sessionId: number;
  path: string;
  line: number | null;
}

export const openPathRequest = writable<OpenPathRequest | null>(null);

export function requestOpenPath(sessionId: number, path: string, line: number | null = null): void {
  openPathRequest.set({ sessionId, path, line });
}

/** Context key under which a markdown host offers a path-open callback. */
export const OPEN_PATH_CONTEXT = 'md-open-path';
export type OpenPathFn = (path: string, line: number | null) => void;

export type AppChord = 'hosts' | 'settings' | 'session-view' | 'agent' | 'scope' | 'today';

/**
 * The app-level chords, platform-correct like the quick switcher's:
 * ⌘I toggles Hosts, ⌘J flips the Session view, ⌘E opens the agent and ⌘,
 * opens Settings (Cmd never reaches the PTY); on non-mac Ctrl+Shift+H,
 * Ctrl+Shift+J and Ctrl+Shift+E do the same (Ctrl+Shift+I is the devtools
 * chord). Plain Ctrl chords stay with the terminal — Ctrl+J is line-feed
 * there. ⌘⇧O / Ctrl+Shift+O cycles the org scope (work graph M5), and
 * ⌘⇧T / Ctrl+Shift+T toggles the Today view over Details (M9.1).
 */
export function appChord(
  e: { key: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean },
  isMac: boolean,
): AppChord | null {
  if (e.altKey) return null;
  const k = e.key.toLowerCase();
  if (k === 'o' && e.shiftKey && (isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey)) {
    return 'scope';
  }
  if (k === 't' && e.shiftKey && (isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey)) {
    return 'today';
  }
  if (e.metaKey && !e.ctrlKey && !e.shiftKey) {
    if (k === 'i') return 'hosts';
    if (k === 'j') return 'session-view';
    if (k === 'e') return 'agent';
    if (k === ',') return 'settings';
    return null;
  }
  if (!isMac && e.ctrlKey && e.shiftKey && !e.metaKey) {
    if (k === 'h') return 'hosts';
    if (k === 'j') return 'session-view';
    if (k === 'e') return 'agent';
  }
  return null;
}

/** Label for the Hosts chord, for the tab and Settings. */
export function hostsChordLabel(isMac: boolean): string {
  return isMac ? '⌘I' : 'Ctrl+Shift+H';
}

/** Label for the Session-view chord, for the segment's tooltip. */
export function sessionViewChordLabel(isMac: boolean): string {
  return isMac ? '⌘J' : 'Ctrl+Shift+J';
}

/** Label for the scope chord, for the selector's tooltip. */
export function scopeChordLabel(isMac: boolean): string {
  return isMac ? '⌘⇧O' : 'Ctrl+Shift+O';
}

/** Label for the Today chord, for the view's tooltip. */
export function todayChordLabel(isMac: boolean): string {
  return isMac ? '⌘⇧T' : 'Ctrl+Shift+T';
}

/** Label for the agent chord, for the FAB's tooltip and the hint. */
export function agentChordLabel(isMac: boolean): string {
  return isMac ? '⌘E' : 'Ctrl+Shift+E';
}
