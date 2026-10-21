// Control's journal and thread mutes (Control review F10, F12;
// `service::control_journal`, `service::control_follow`). The hub keeps what
// Control follows, what it asked the person and what they last said, and
// hands it to every new Control conversation. Here the threads column reads
// which threads are muted and mutes or unmutes one: a muted thread is one
// fleet stops telling Control about. Control's escalation cards read which
// threads are still followed (`threadFollows`).
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
  action?: 'read' | 'note' | 'ask' | 'mute' | 'unmute' | 'follow';
  text?: string;
  thread?: 'session' | 'mission';
  id?: number;
}

export function controlJournal<T = unknown>(args: JournalArgs): Promise<Result<T>> {
  return invokeCmd<T>('control_journal', { args });
}

/** Whether the hub still follows a thread, and whether it is muted. */
export interface FollowMark {
  state: string;
  muted: boolean;
  /** A session's tmux name, a mission's name (the column's fallback title). */
  name?: string;
}

/** Every thread of the last journal read by key (`s:<id>`, `m:<id>`);
 *  `null` until one is read, or from an older hub. */
export const threadFollows = writable<ReadonlyMap<string, FollowMark> | null>(null);

/** PURE: a journal read's threads by key, `null` from an older hub. */
export function followMarks(j: ControlJournal | null | undefined): Map<string, FollowMark> | null {
  if (!j) return null;
  const out = new Map<string, FollowMark>();
  for (const f of j.follows ?? []) out.set(`${f.kind === 'session' ? 's' : 'm'}:${f.id}`, { state: f.state, muted: f.muted, name: f.name });
  return out;
}

/** PURE: whether a mark (none: the hub did not say) leaves its thread followed. */
export function stillFollowed(mark: FollowMark | undefined): boolean {
  return !mark || (!mark.muted && mark.state === 'following');
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
  if (r.ok) {
    mutedThreads.set(mutedKeys(r.value));
    threadFollows.set(followMarks(r.value));
  }
}

/** Mute or unmute one thread, then re-read. */
export async function setThreadMuted(kind: 'session' | 'mission', id: number, on: boolean): Promise<Result<unknown>> {
  const r = await controlJournal({ action: on ? 'mute' : 'unmute', thread: kind, id });
  if (r.ok) await refreshMutes();
  return r;
}
