// A tiny cross-component request: "open the new-session dialog for this
// project (optionally with a pre-filled name)". The Sidebar mounts its own
// NewSessionDialog for its footer button; the quick switcher lives in
// App.svelte and cannot reach into the Sidebar, so it publishes a request
// here and App.svelte mounts a second dialog instance. Once #46 lands the
// Sidebar can switch to this store too and the duplicate mount goes away.
import { writable } from 'svelte/store';
import type { ProjectTreeRow } from './projects';

export interface NewSessionRequest {
  project: ProjectTreeRow;
  /** Pre-fill the friendly-name field (e.g. the switcher's query). */
  initialName?: string;
}

export const newSessionRequest = writable<NewSessionRequest | null>(null);

export function requestNewSession(req: NewSessionRequest): void {
  newSessionRequest.set(req);
}

export function clearNewSessionRequest(): void {
  newSessionRequest.set(null);
}
