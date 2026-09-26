// The agent sheet's size. It used to be a fixed 360px column capped at 60vh,
// which is a fine default for a quick question and useless for reading a
// long answer. The person can now drag its top-left corner (the sheet is
// anchored bottom-right, so that is the corner that moves) or maximize it,
// and the size outlives a restart like the sidebar width does.
//
// `null` is the default: the sheet sizes to its content under the 60vh cap.
// A stored size is clamped here to sane minimums; the viewport cap is CSS
// (`min(…, 100vw - …)`), so a size saved on a big screen never pushes the
// sheet off a smaller one.

import { writable } from 'svelte/store';
import { readPref, writePref, clearPref } from './prefs';

export interface AgentPanelSize {
  w: number;
  h: number;
}

export const AGENT_PANEL_MIN_W = 300;
export const AGENT_PANEL_MIN_H = 240;
/** Where a keyboard resize or a first drag starts from, matching the CSS default. */
export const AGENT_PANEL_DEFAULT_W = 360;

const PREF_KEY = 'agent.panelSize';

export function isAgentPanelSize(v: unknown): v is AgentPanelSize {
  if (typeof v !== 'object' || v === null) return false;
  const s = v as Record<string, unknown>;
  return (
    typeof s.w === 'number' && Number.isFinite(s.w) && typeof s.h === 'number' && Number.isFinite(s.h)
  );
}

/** Raise a size to the minimums and round it to whole pixels. */
export function clampAgentPanelSize(s: AgentPanelSize, max?: AgentPanelSize): AgentPanelSize {
  let w = Math.max(AGENT_PANEL_MIN_W, Math.round(s.w));
  let h = Math.max(AGENT_PANEL_MIN_H, Math.round(s.h));
  if (max) {
    w = Math.min(w, Math.max(AGENT_PANEL_MIN_W, Math.floor(max.w)));
    h = Math.min(h, Math.max(AGENT_PANEL_MIN_H, Math.floor(max.h)));
  }
  return { w, h };
}

/** A drag of the top-left corner: moving it left / up grows the sheet. */
export function dragResize(start: AgentPanelSize, dx: number, dy: number, max?: AgentPanelSize): AgentPanelSize {
  return clampAgentPanelSize({ w: start.w - dx, h: start.h - dy }, max);
}

export const agentPanelSize = writable<AgentPanelSize | null>(readPref(PREF_KEY, null, isAgentPanelSizeOrNull));
agentPanelSize.subscribe((v) => (v ? writePref(PREF_KEY, v) : clearPref(PREF_KEY)));

function isAgentPanelSizeOrNull(v: unknown): v is AgentPanelSize | null {
  return v === null || isAgentPanelSize(v);
}

/** Maximized is a flag, not a size: it follows the window, and leaving it
 *  returns to whatever size the sheet had before. */
export const agentPanelMaximized = writable<boolean>(
  readPref('agent.panelMaximized', false, (v): v is boolean => typeof v === 'boolean'),
);
agentPanelMaximized.subscribe((v) => writePref('agent.panelMaximized', v));
