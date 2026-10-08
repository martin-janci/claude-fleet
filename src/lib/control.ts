// Control (Orbit Fleet redesign step 9.1): the New layout's rail item for the
// fleet agent. The operator panel moves out of its floating sheet into the
// right column, and Today moves here from the Inbox as Control's second tab.
// ⌘E opens Control and ⌘⇧T its Today tab, in the New layout only; Classic
// keeps the sheet (⌘E) and Today over Details (⌘⇧T) exactly as before.
import { get, writable } from 'svelte/store';
import { destination, goTo, leave } from './destination';
import { uiLayout } from './prefs';
import { todayOpen } from './today';

export type ControlTab = 'chat' | 'today';

/** Which tab Control shows. Kept across visits, as a tab is. */
export const controlTab = writable<ControlTab>('chat');

/** Open Control at `tab`. */
export function openControl(tab: ControlTab = 'chat'): void {
  controlTab.set(tab);
  goTo('control');
}

/** A chord's toggle: open Control at `tab`, or leave it when that tab is
 *  already showing (the second press closes, as the sheet's ⌘E did). */
export function toggleControl(tab: ControlTab): void {
  if (get(destination) === 'control' && get(controlTab) === tab) leave('control');
  else openControl(tab);
}

/** ⌘⇧T and the switcher's Today row: Control's Today tab in the New layout,
 *  Today over Details in Classic. */
export function toggleToday(): void {
  if (get(uiLayout) === 'new') toggleControl('today');
  else todayOpen.update((v) => !v);
}

export function openToday(): void {
  if (get(uiLayout) === 'new') openControl('today');
  else todayOpen.set(true);
}
