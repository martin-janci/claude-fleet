// "Open the switcher in New session mode" (project picker spec v2) — from
// the sidebar's "+ New session" button, which cannot reach the switcher
// mounted in App.svelte.
import { writable } from 'svelte/store';
import type { TicketRow } from './trackers';

export interface SwitcherRequest {
  /** 'switch' is the plain ⌘K switcher, for the header's command field (3.17). */
  mode: 'new' | 'switch';
  host?: string;
  /** Pick the repository for this ticket's start (redesign 3.12: the
   *  New session dialog's "Change" on a proposed project). The chosen
   *  project opens the dialog with the ticket still attached. */
  ticket?: TicketRow;
  /** Preselect what the session runs (gap plan G4.5: the Hosts view's
   *  "Open a shell"). */
  kind?: 'work' | 'shell';
}

export const switcherRequest = writable<SwitcherRequest | null>(null);

export function openNewSessionPicker(host?: string, ticket?: TicketRow, kind?: 'work' | 'shell'): void {
  switcherRequest.set({ mode: 'new', host, ticket, kind });
}

/** Open the plain switcher, as ⌘K does (the header's command field). */
export function openSwitcher(): void {
  switcherRequest.set({ mode: 'switch' });
}
