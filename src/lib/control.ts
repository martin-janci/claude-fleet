// Control (Orbit Fleet redesign step 9.1): the rail item for the
// fleet agent. ⌘E opens Control and ⌘⇧T its Today view.
//
// Thread room (2026-10-14): Control has no tabs of its own any more. The
// chat is the page (Main chat in the left column), and Today is a view of
// the right panel; `controlTab` says which of the two the last chord asked
// for, so ⌘⇧T toggles Today and ⌘E toggles Control.
import { get, writable } from 'svelte/store';
import { destination, goTo, leave } from './destination';
import { controlViews, selectView } from './control_views';

export type ControlTab = 'chat' | 'today';

/** What the last chord or link asked Control for. */
export const controlTab = writable<ControlTab>('chat');

/** Open Control; `today` also puts the Today view on the right. */
export function openControl(tab: ControlTab = 'chat'): void {
  controlTab.set(tab);
  if (tab === 'today') selectView('today');
  goTo('control');
}

/** Whether `tab` is what Control shows now: the chat whenever Control is
 *  open, Today while the right panel shows it. */
export function controlShowing(tab: ControlTab): boolean {
  if (get(destination) !== 'control') return false;
  if (tab === 'chat') return true;
  const l = get(controlViews);
  return l.open && l.active === 'today';
}

/** A chord's toggle: open Control at `tab`, or leave it when that is
 *  already showing (the second press closes, as the sheet's ⌘E did). */
export function toggleControl(tab: ControlTab): void {
  if (controlShowing(tab)) leave('control');
  else openControl(tab);
}

/** ⌘⇧T and the switcher's Today row: Control's Today tab. */
export function toggleToday(): void {
  toggleControl('today');
}

export function openToday(): void {
  openControl('today');
}
