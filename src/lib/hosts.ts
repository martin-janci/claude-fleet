import { writable, derived } from 'svelte/store';
import { createRowStore } from './row_store';
import { invokeCmd, type Result } from './result';
import { readPref, writePref } from './prefs';

export interface HostRow {
  alias: string;
  ssh_alias: string | null;
  reachable: boolean;
  claude_version: string | null;
  tmux_version: string | null;
  hidden: boolean;
  last_pinged_at: number | null;
  account_uuid: string | null;
  provisioned: boolean;
  /** `"ssh"` | `"agent"` — see `HostRow::transport` in `crates/fleet-core/src/store/rows.rs`. */
  transport: 'ssh' | 'agent';
  /** The host's org (work graph M5): its per-host token's boundary. */
  org_id?: number | null;
}

export interface SshHost {
  alias: string;
  hostname: string | null;
  user: string | null;
  port: number | null;
}

const rows = createRowStore<HostRow, string>({
  key: (h) => h.alias,
  // `removeHost()` (optimistic) and the `host:removed` event both delete a
  // row; a `host:probed` event still in flight for that alias would otherwise
  // re-insert the dead host. Entries expire so re-adding a host with the
  // same alias isn't blocked for long.
  tombstoneMs: 5000,
});
export const hosts = rows.store;
export const resetTombstonesForTests = rows.resetTombstonesForTests;

/** O(1) alias -> host lookup, derived once per `hosts` change. Consumers
 *  that previously did a linear `$hosts.find` should read this instead. */
export const hostByAlias = derived(hosts, ($h) => new Map($h.map((h) => [h.alias, h])));

/** Whether a new session / project may target `alias`: visible, and
 *  reachable unless it is `local`. The rule behind HostChips' `disabled`
 *  and every new-thing dialog's host choice. */
export function isPickableHost(list: readonly HostRow[], alias: string | null | undefined): alias is string {
  return !!alias && list.some((h) => h.alias === alias && !h.hidden && (h.reachable || h.alias === 'local'));
}

/** The host a dialog starts on when nothing remembered is pickable: `local`
 *  when it is, else the first pickable host. A hub started with
 *  `hub.local_host=false` hides its `local` row, and defaulting to it there
 *  only earns "host local is disabled on this hub" on submit. `'local'` when
 *  nothing at all is pickable (the submit then says why). */
export function defaultHost(list: readonly HostRow[]): string {
  if (isPickableHost(list, 'local')) return 'local';
  return list.find((h) => isPickableHost(list, h.alias))?.alias ?? 'local';
}

// Sidebar host filter — `'all'` shows sessions from every host, otherwise
// the value is a specific `alias`. Persisted across restarts.
const isString = (v: unknown): v is string => typeof v === 'string';
export const hostFilter = writable<string>(readPref('host-filter', 'all', isString));
hostFilter.subscribe((v) => writePref('host-filter', v));

export async function loadHosts(): Promise<Result<HostRow[]>> {
  const r = await invokeCmd<HostRow[]>('list_hosts');
  if (r.ok) hosts.set(r.value);
  return r;
}

export async function discoverHosts(): Promise<Result<SshHost[]>> {
  return invokeCmd<SshHost[]>('discover_hosts');
}

export async function addHost(
  alias: string,
  sshAlias: string,
): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('add_host', {
    args: { alias, ssh_alias: sshAlias },
  });
  if (r.ok) {
    // An explicit re-add overrides any lingering tombstone from a recent
    // removeHost() of the same alias.
    rows.accept(r.value);
  }
  return r;
}

export async function probeHost(alias: string): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('probe_host', { args: { alias } });
  // A command result is authoritative — clear any stale tombstone first.
  if (r.ok) {
    rows.accept(r.value);
  }
  return r;
}

export async function deleteHost(alias: string): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('remove_host', { args: { alias } });
  if (r.ok) removeHost(r.value.alias);
  return r;
}

export async function hideHost(
  alias: string,
  hidden: boolean,
): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('hide_host', { args: { alias, hidden } });
  if (r.ok) {
    rows.accept(r.value);
  }
  return r;
}

function removeHost(alias: string): void {
  rows.remove(alias);
}

/** One backend host event, as delivered by `events.ts`. */
export type HostEvent =
  | { type: 'added' | 'probed'; row: HostRow }
  /** A probe that changed nothing but the stamp — patched onto the row we hold. */
  | { type: 'pinged'; alias: string; last_pinged_at: number; reachable: boolean }
  | { type: 'removed'; alias: string };

/** Apply a burst of host events in ONE store update, in order (see
 *  `applySessionEvents` for the rationale). */
export function applyHostEvents(events: readonly HostEvent[]): void {
  if (events.length === 0) return;
  hosts.update((arr) => {
    let next = arr;
    for (const ev of events) {
      if (ev.type === 'removed') {
        next = rows.removeFrom(next, ev.alias);
      } else if (ev.type === 'pinged') {
        // A partial event, so it patches rather than replaces — and only a
        // row we already hold: a heartbeat is never the first we hear of a
        // host, and inventing one from three fields would show a host with
        // no versions and no transport.
        const have = next.find((h) => h.alias === ev.alias);
        if (have) {
          next = rows.mergeInto(next, {
            ...have,
            last_pinged_at: ev.last_pinged_at,
            reachable: ev.reachable,
          });
        }
      } else {
        next = rows.mergeInto(next, ev.row);
      }
    }
    return next;
  });
}
