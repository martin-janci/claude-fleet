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

/** Settings dialog visibility (mounted by the Sidebar; ⌘, sets it). */
export const settingsOpen = writable(false);

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

export type AppChord = 'hosts' | 'settings' | 'session-view';

/**
 * The app-level chords, platform-correct like the quick switcher's:
 * ⌘I toggles Hosts, ⌘J flips the Session view and ⌘, opens Settings (Cmd
 * never reaches the PTY); on non-mac Ctrl+Shift+H and Ctrl+Shift+J do the
 * same two views (Ctrl+Shift+I is the devtools chord). Plain Ctrl chords
 * stay with the terminal — Ctrl+J is line-feed there.
 */
export function appChord(
  e: { key: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean },
  isMac: boolean,
): AppChord | null {
  if (e.altKey) return null;
  const k = e.key.toLowerCase();
  if (e.metaKey && !e.ctrlKey && !e.shiftKey) {
    if (k === 'i') return 'hosts';
    if (k === 'j') return 'session-view';
    if (k === ',') return 'settings';
    return null;
  }
  if (!isMac && e.ctrlKey && e.shiftKey && !e.metaKey) {
    if (k === 'h') return 'hosts';
    if (k === 'j') return 'session-view';
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
