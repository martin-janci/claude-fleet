// "Open the switcher in New session mode" (project picker spec v2) — from
// the sidebar's "+ New session" button, which cannot reach the switcher
// mounted in App.svelte.
import { writable } from 'svelte/store';
import type { TicketRow } from './trackers';

export interface SwitcherRequest {
  mode: 'new';
  host?: string;
  /** Pick the repository for this ticket's start (redesign 3.12: the
   *  New session dialog's "Change" on a proposed project). The chosen
   *  project opens the dialog with the ticket still attached. */
  ticket?: TicketRow;
}

export const switcherRequest = writable<SwitcherRequest | null>(null);

export function openNewSessionPicker(host?: string, ticket?: TicketRow): void {
  switcherRequest.set({ mode: 'new', host, ticket });
}
