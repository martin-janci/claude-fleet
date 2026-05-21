import { writable } from 'svelte/store';
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
}

export interface SshHost {
  alias: string;
  hostname: string | null;
  user: string | null;
  port: number | null;
}

export const hosts = writable<HostRow[]>([]);

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
    hostTombstones.delete(alias);
    mergeHost(r.value);
  }
  return r;
}

export async function probeHost(alias: string): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('probe_host', { args: { alias } });
  // A command result is authoritative — clear any stale tombstone first.
  if (r.ok) {
    hostTombstones.delete(alias);
    mergeHost(r.value);
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
    hostTombstones.delete(alias);
    mergeHost(r.value);
  }
  return r;
}

export async function bootstrapHosts(): Promise<void> {
  const r = await invokeCmd<HostRow[]>('list_hosts');
  if (r.ok) hosts.set(r.value);
}

// Recently-removed host aliases. `removeHost()` (optimistic) and the
// `host:removed` event both delete a row; a `host:probed` event still in
// flight for that alias would otherwise re-insert the dead host. Entries
// expire so re-adding a host with the same alias isn't blocked for long.
const hostTombstones = new Map<string, number>();
const HOST_TOMBSTONE_MS = 5000;

function isHostTombstoned(alias: string): boolean {
  const t = hostTombstones.get(alias);
  if (t === undefined) return false;
  if (Date.now() - t > HOST_TOMBSTONE_MS) {
    hostTombstones.delete(alias);
    return false;
  }
  return true;
}

export function mergeHost(row: HostRow): void {
  if (isHostTombstoned(row.alias)) return;
  hosts.update((arr) => {
    const i = arr.findIndex((h) => h.alias === row.alias);
    if (i === -1) return [...arr, row];
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

export function removeHost(alias: string): void {
  hostTombstones.set(alias, Date.now());
  hosts.update((arr) => arr.filter((h) => h.alias !== alias));
}
