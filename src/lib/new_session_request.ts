// A tiny cross-component request: "open the new-session dialog for this
// project (optionally with a pre-filled name)". The quick switcher (mounted in
// App.svelte) is the one place a project is picked for a new session — from
// its New session mode, opened by the sidebar's "+ New session" and the Hosts
// view's `n` — and it cannot reach into the Sidebar, so it publishes a request
// here and App.svelte mounts the dialog. It is the only mount (redesign 1.9):
// a project row's `+` and the end of Add project publish the same request.
import { writable } from 'svelte/store';
import type { ProjectTreeRow } from './projects';
import type { TicketRow } from './trackers';

export interface NewSessionRequest {
  project: ProjectTreeRow;
  /** Pre-fill the friendly-name field (e.g. the switcher's query). */
  initialName?: string;
  /** Preselect this host. */
  initialHost?: string;
  /** Start work on this ticket (work graph M3): the dialog offers the
   *  "Brief Claude with the ticket" preview, and creating links it. */
  ticket?: TicketRow;
  /** Start at once with the remembered choices (the picker's ⌘↵); the
   *  dialog stays open only if something needs a person. */
  autostart?: boolean;
}

export const newSessionRequest = writable<NewSessionRequest | null>(null);

export function requestNewSession(req: NewSessionRequest): void {
  newSessionRequest.set(req);
}

export function clearNewSessionRequest(): void {
  newSessionRequest.set(null);
}
