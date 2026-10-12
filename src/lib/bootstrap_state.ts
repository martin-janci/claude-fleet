import { writable } from 'svelte/store';
import type { IpcError } from './result';

/**
 * Review round 13: the startup list loads (sessions, projects) failed.
 * App.svelte sets it when its bootstrap reads fail and clears it when they
 * succeed; the Sessions list and the Inbox read it so a failed load is said
 * as a failure ("Couldn't load sessions · Retry"), never as a first run
 * ("No projects yet") or a calm Inbox ("Nothing needs you").
 */
export const bootstrapError = writable<IpcError | null>(null);
