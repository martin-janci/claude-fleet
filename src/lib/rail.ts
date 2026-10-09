// The rail (Orbit Fleet redesign step 3.2): the design manual's
// order, Control, Inbox, Sessions, Work, Automation, Accounts, Toolkit, then
// Settings at the bottom. An item whose step has not landed stays hidden;
// flipping `landed` (with its click handler in `AppRail.svelte`) is how that
// step adds it.
import type { Destination } from './destination';
import type { SidebarView } from './work_view';

export type RailId =
  | 'control'
  | 'inbox'
  | 'sessions'
  | 'work'
  | 'automation'
  | 'accounts'
  | 'toolkit'
  | 'settings';

export interface RailItem {
  id: RailId;
  label: string;
  /** The `shortcuts.ts` row its title names, when it has one. */
  shortcut: string | null;
  /** The plan step that brings it; it stays hidden until then. */
  step: string;
  landed: boolean;
}

export const RAIL_ITEMS: readonly RailItem[] = [
  { id: 'control', label: 'Control', shortcut: 'agent', step: '9.1', landed: true },
  { id: 'inbox', label: 'Inbox', shortcut: null, step: '3.3', landed: true },
  { id: 'sessions', label: 'Sessions', shortcut: 'work-view', step: '3.2', landed: true },
  { id: 'work', label: 'Work', shortcut: 'work-view', step: '3.2', landed: true },
  { id: 'automation', label: 'Automation', shortcut: null, step: '8.4', landed: false },
  { id: 'accounts', label: 'Accounts', shortcut: 'hosts', step: '4.1', landed: true },
  { id: 'toolkit', label: 'Toolkit', shortcut: null, step: '3.16', landed: true },
  { id: 'settings', label: 'Settings', shortcut: 'settings', step: '3.2', landed: true },
];

/** The items the rail shows, in order. */
export function visibleRailItems(items: readonly RailItem[] = RAIL_ITEMS): RailItem[] {
  return items.filter((i) => i.landed);
}

/**
 * The item the right column belongs to. Accounts covers both of its pages
 * (Accounts and Hosts: "Accounts & hosts"); every other overlay (Files, the
 * board) sits over a session, so the sidebar's tree names the item. Assets
 * is Toolkit's screen (3.16). The Inbox (3.3) is the Sessions list narrowed,
 * so the same rule names it. Control (9.1) is a destination of its own.
 */
export function currentRailItem(dest: Destination, view: SidebarView): RailId | null {
  if (dest === 'control') return 'control';
  if (dest === 'accounts' || dest === 'hosts') return 'accounts';
  if (dest === 'assets') return 'toolkit';
  return view === 'work' ? 'work' : view === 'inbox' ? 'inbox' : 'sessions';
}
