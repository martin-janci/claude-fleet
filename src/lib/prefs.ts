/**
 * Tiny localStorage-backed key/value store for global UI prefs.
 *
 * Per-app prefs (sidebar width, last-used filter), not tied to any specific
 * tmux session.
 */

import { writable } from 'svelte/store';
import { detectMac } from './terminal_keys';
import type { SessionView } from './session_view';

const PREFIX = 'cf:pref:';

export function readPref<T>(key: string, fallback: T, isValid: (v: unknown) => v is T): T {
  if (typeof localStorage === 'undefined') return fallback;
  try {
    const raw = localStorage.getItem(PREFIX + key);
    if (raw === null) return fallback;
    const parsed = JSON.parse(raw);
    return isValid(parsed) ? parsed : fallback;
  } catch {
    return fallback;
  }
}

export function writePref<T>(key: string, value: T): void {
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    /* quota — silently degrade */
  }
}

/** Forget a pref so the next read falls back to its default. */
export function clearPref(key: string): void {
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.removeItem(PREFIX + key);
  } catch {
    /* ignore */
  }
}

// ─── Terminal prefs ──────────────────────────────────────────────────────

const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

/**
 * Copy a drag-selection to the clipboard as soon as the mouse is released
 * (X11 "primary selection" habit). Defaults on for macOS to keep the
 * behaviour the app shipped with there, off elsewhere — on Linux/Windows a
 * drag would otherwise silently overwrite whatever the user just copied.
 * Explicit copy (Cmd+C / Ctrl+Shift+C / context menu) always works.
 */
export const copyOnSelect = writable<boolean>(
  readPref(
    'terminal.copyOnSelect',
    detectMac(typeof navigator === 'undefined' ? undefined : navigator),
    isBool,
  ),
);
copyOnSelect.subscribe((v) => writePref('terminal.copyOnSelect', v));

// ─── Right-panel prefs ───────────────────────────────────────────────────

const isSessionView = (v: unknown): v is SessionView =>
  v === 'conversation' || v === 'terminal';

/**
 * Which sub-view the Session tab shows. One choice for the whole app, kept
 * across restarts — picking a session should not decide for you which of
 * its two views you get. Rows that can only offer one view override this
 * without writing to it; see `resolveSessionView`.
 */
export const sessionView = writable<SessionView>(
  readPref<SessionView>('ui.sessionView', 'conversation', isSessionView),
);
sessionView.subscribe((v) => writePref('ui.sessionView', v));

// ─── Retired layout pref ─────────────────────────────────────────────────

// Step 13.1 removed the Classic layout, and with it the Classic/New switch
// and the center Details pane. Their keys (`ui.layout`, then `ui.layout.v2`;
// the pane's collapsed state, and its per-session widths under
// `cf:session-ui`; the Inbox Classic stood in for) are forgotten once at
// load.
clearPref('ui.layout');
clearPref('ui.layout.v2');
clearPref('layout.center-collapsed');
clearPref('sidebar.inbox-before-classic');
try {
  if (typeof localStorage !== 'undefined') localStorage.removeItem('cf:session-ui');
} catch {
  /* ignore */
}

// ─── Row density (redesign step 3.6) ─────────────────────────────────────

export type UiDensity = 'comfortable' | 'compact';

const isUiDensity = (v: unknown): v is UiDensity => v === 'comfortable' || v === 'compact';

/**
 * How tall a session row is. Comfortable is 0.5.4's row, every badge
 * included; Compact is the redesign's two-line row (a sans title, one meta
 * line, chips until hover) at {@link COMPACT_ROW_PX}.
 */
// Compact is the default since the UX audit (2026-10-09, L1). The key moved
// from `ui.density` because the store writes its value back on first read,
// so every install already held 'comfortable' without anyone choosing it;
// a fresh key gives everyone the design's row once, and Settings › Appearance
// still switches back.
export const uiDensity = writable<UiDensity>(readPref<UiDensity>('ui.rowDensity', 'compact', isUiDensity));
uiDensity.subscribe((v) => writePref('ui.rowDensity', v));

/** A Compact row's height: what `.sess-row.compact`'s min-height
 *  (`calc(2 * var(--text-2xs-lh) + var(--space-3))`) resolves to. 20 of
 *  them fit a 1080p window with the shell's chrome (SessionRowDensity.test.ts
 *  renders and measures them). */
export const COMPACT_ROW_PX = 40;

// ─── Agent tab name and badge counts (gap plan G4.6) ─────────────────────

export type AgentTabName = 'agent' | 'terminal';

const isAgentTabName = (v: unknown): v is AgentTabName => v === 'agent' || v === 'terminal';

/**
 * What the session's agent tab is called. `agent` (the default, step 5.1)
 * names it after the agent in it, with its mark: "Claude Code", "Codex";
 * `terminal` calls every one "Terminal", with no mark.
 */
export const agentTabName = writable<AgentTabName>(readPref<AgentTabName>('ui.agentTabName', 'agent', isAgentTabName));
agentTabName.subscribe((v) => writePref('ui.agentTabName', v));

/** The agent tab's label under `pref`; `agent` is the agent's own name, or
 *  null when the row names none. */
export function agentTabLabel(agent: string | null | undefined, pref: AgentTabName): string {
  return pref === 'terminal' || !agent ? 'Terminal' : agent;
}

export type BadgeCounts = 'needs_you' | 'off';

const isBadgeCounts = (v: unknown): v is BadgeCounts => v === 'needs_you' || v === 'off';

/**
 * What raises the Inbox badge (the rail's count and the dock's). `needs_you`
 * (the default) counts only Waiting, Failed and Blocked, never a ghost or
 * lost row; `off` shows no number anywhere. The Inbox itself is unchanged.
 */
export const badgeCounts = writable<BadgeCounts>(readPref<BadgeCounts>('ui.badgeCounts', 'needs_you', isBadgeCounts));
badgeCounts.subscribe((v) => writePref('ui.badgeCounts', v));

/** The badge's number under `pref`. */
export function badgeNumber(count: number, pref: BadgeCounts): number {
  return pref === 'off' ? 0 : count;
}
