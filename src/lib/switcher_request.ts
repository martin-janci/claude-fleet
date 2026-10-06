// "Open the switcher in New session mode" (project picker spec v2) — from
// the sidebar's "+ New session" button, which cannot reach the switcher
// mounted in App.svelte.
import { writable } from 'svelte/store';

export const switcherRequest = writable<{ mode: 'new'; host?: string } | null>(null);

export function openNewSessionPicker(host?: string): void {
  switcherRequest.set({ mode: 'new', host });
}
