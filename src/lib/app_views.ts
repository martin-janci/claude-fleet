// App-level view requests that cross component boundaries. App.svelte owns
// the Hosts mode (it shares the Files-mode overlay, so the terminal stays
// mounted) and the Sidebar owns Settings and the project picker; the quick
// switcher, Settings, the onboarding card and the Hosts view reach them
// through these stores instead of prop-drilling. Same pattern as
// `new_session_request.ts`.
import { matchShortcut, shortcutLabel, type KeyEventLike } from './shortcuts';
import { writable } from 'svelte/store';
import { destinationFlag } from './destination';
import type { AssetsCommand } from './quick_switcher';
import type { AssetKind } from './assets';

export interface HostsViewRequest {
  /** Host alias to preselect; `null` lets App pick (selected session's host,
   *  else the last-viewed host, else the view's own default). */
  host: string | null;
}

/** A pending "open the Hosts view" request; App consumes and clears it. */
export const hostsViewRequest = writable<HostsViewRequest | null>(null);

/** Whether the Hosts view is showing: a view of the `destination` store
 *  (redesign step 3.1), which App writes. */
export const hostsViewOpen = destinationFlag('hosts');

export function requestHostsView(host: string | null = null): void {
  hostsViewRequest.set({ host });
}

/** A pending "open the Assets view" request (the quick switcher's asset and
 *  command rows, Assets M6 R19): App opens the overlay, `AssetsPanel` takes
 *  it, selects `select` (a workspace row key) or runs `command`, and clears
 *  it. It outlives the panel's mount: the panel mounts only while the
 *  overlay is open, so a request set first is read on mount. `at` makes two
 *  identical requests distinct. */
export interface AssetsViewRequest {
  select?: string;
  command?: AssetsCommand;
  /** Open New asset on this kind (M15 G7.13: Toolkit's "+ Add skill"). */
  newKind?: AssetKind;
  at: number;
}

export const assetsViewRequest = writable<AssetsViewRequest | null>(null);

export function requestAssetsView(r: Omit<AssetsViewRequest, 'at'>): void {
  assetsViewRequest.set({ ...r, at: Date.now() });
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

/** The `?` keyboard-shortcut sheet (redesign step 3.8; ⌘K opens it too). */
export const shortcutSheetOpen = writable(false);

/** The Settings section to scroll to when the dialog opens (`work`: the
 *  work section, e.g. from a "Reconnect Jira (acme)" Attention item). The
 *  dialog clears it once it has scrolled. */
export const settingsSection = writable<string | null>(null);

/** The setting to show on the page `settingsSection` names (a chat card's
 *  "undo in Settings"). Cleared with the section. */
export const settingsKey = writable<string | null>(null);

/** Open Settings at `section`; on a page, at `key` when given. */
export function openSettingsAt(section: string, key?: string): void {
  settingsKey.set(key ?? null);
  settingsSection.set(section);
  settingsOpen.set(true);
}

/**
 * "Start a new session on this host" (the Hosts view's `n`): the quick
 * switcher opens in New session mode preferring that host, and preselects
 * it in the NewSessionDialog that follows.
 */
export const newSessionHostRequest = writable<string | null>(null);

export function requestNewSessionOnHost(host: string): void {
  newSessionHostRequest.set(host);
}

/** "Open Add project" from the switcher's Add row; the Sidebar owns the
 *  dialog. `cloneUrl` prefills the Clone URL field. */
export const addProjectRequest = writable<{ cloneUrl?: string } | null>(null);

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

export type AppChord =
  | 'hosts'
  | 'settings'
  | 'session-view'
  | 'agent'
  | 'scope'
  | 'today'
  | 'work-view'
  | 'inspector'
  | 'open-in-editor';

const APP_CHORDS: readonly AppChord[] = [
  'hosts',
  'settings',
  'session-view',
  'agent',
  'scope',
  'today',
  'work-view',
  'inspector',
  'open-in-editor',
];

/**
 * The app-level chords, platform-correct like the quick switcher's:
 * ⌘I toggles Hosts, ⌘J flips the Session view, ⌘E opens the agent and ⌘,
 * opens Settings (Cmd never reaches the PTY); on non-mac Ctrl+Shift+H,
 * Ctrl+Shift+J and Ctrl+Shift+E do the same (Ctrl+Shift+I is the devtools
 * chord). Plain Ctrl chords stay with the terminal — Ctrl+J is line-feed
 * there — except Ctrl+, for Settings (redesign 1.9), which no terminal
 * program reads. ⌘⇧O / Ctrl+Shift+O cycles the org scope (work graph M5), and
 * ⌘⇧T / Ctrl+Shift+T toggles the Today view over Details (M9.1), and
 * ⌘⇧W / Ctrl+Shift+W flips the sidebar between Sessions and Work (M14).
 * The chords themselves live in the shortcut registry (`shortcuts.ts`).
 */
export function appChord(e: KeyEventLike, isMac: boolean): AppChord | null {
  const id = matchShortcut('global', e, isMac);
  return APP_CHORDS.includes(id as AppChord) ? (id as AppChord) : null;
}

/** Label for the Hosts chord, for the tab and Settings. */
export function hostsChordLabel(isMac: boolean): string {
  return shortcutLabel('hosts', isMac);
}

/** Label for the Session-view chord, for the segment's tooltip. */
export function sessionViewChordLabel(isMac: boolean): string {
  return shortcutLabel('session-view', isMac);
}

/** Label for the scope chord, for the selector's tooltip. */
export function scopeChordLabel(isMac: boolean): string {
  return shortcutLabel('scope', isMac);
}

/** Label for the Today chord, for the view's tooltip. */
export function todayChordLabel(isMac: boolean): string {
  return shortcutLabel('today', isMac);
}

/** Label for the agent chord, for the FAB's tooltip and the hint. */
export function agentChordLabel(isMac: boolean): string {
  return shortcutLabel('agent', isMac);
}

/** Label for the Work-view chord, for the switch's tooltip. */
export function workViewChordLabel(isMac: boolean): string {
  return shortcutLabel('work-view', isMac);
}

/** Whether the task board shows over the terminal (sprints design
 *  2026-09-28 §6c). The Work view's Board button opens it; another overlay
 *  (Files, Assets, Hosts) or the Session tab taking the slot closes it, as
 *  does Esc. A view of the `destination` store (redesign step 3.1): opening
 *  it leaves whichever overlay was open, closing it returns to the Session
 *  tab. */
export const workBoardOpen = destinationFlag('board');
