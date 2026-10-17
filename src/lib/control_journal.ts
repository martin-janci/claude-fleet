// Control's journal and thread mutes (Control review F10, F12;
// `service::control_journal`, `service::control_follow`). The hub keeps what
// Control follows, what it asked the person and what they last said, and
// hands it to every new Control conversation. Here the threads column reads
// which threads are muted and mutes or unmutes one: a muted thread is one
// fleet stops telling Control about.
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

/** A thread Control started or was handed, and whether it is followed. */
export interface JournalFollow {
  kind: 'session' | 'mission';
  id: number;
  /** following | muted | killed | merged | done | quiet */
  state: string;
  name: string;
  muted: boolean;
}

export interface ControlJournal {
  follows: JournalFollow[];
}

export interface JournalArgs {
  action?: 'read' | 'note' | 'ask' | 'mute' | 'unmute';
  text?: string;
  thread?: 'session' | 'mission';
  id?: number;
}

export function controlJournal<T = unknown>(args: JournalArgs): Promise<Result<T>> {
  return invokeCmd<T>('control_journal', { args });
}

/** The muted threads' keys, `s:<id>` and `m:<id>` (`control_threads`). */
export const mutedThreads = writable<ReadonlySet<string>>(new Set());

/** PURE: the muted keys of a journal read (`null` from an older hub). */
export function mutedKeys(j: ControlJournal | null | undefined): Set<string> {
  const out = new Set<string>();
  for (const f of j?.follows ?? []) if (f.muted) out.add(`${f.kind === 'session' ? 's' : 'm'}:${f.id}`);
  return out;
}

/** Re-read the mutes; a failed read keeps what is shown. */
export async function refreshMutes(): Promise<void> {
  const r = await controlJournal<ControlJournal | null>({ action: 'read' });
  if (r.ok) mutedThreads.set(mutedKeys(r.value));
}

/** Mute or unmute one thread, then re-read. */
export async function setThreadMuted(kind: 'session' | 'mission', id: number, on: boolean): Promise<Result<unknown>> {
  const r = await controlJournal({ action: on ? 'mute' : 'unmute', thread: kind, id });
  if (r.ok) await refreshMutes();
  return r;
}
