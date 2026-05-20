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
  if (r.ok) await loadHosts();
  return r;
}

export async function probeHost(alias: string): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('probe_host', { args: { alias } });
  if (r.ok) await loadHosts();
  return r;
}

export async function removeHost(alias: string): Promise<Result<void>> {
  const r = await invokeCmd<void>('remove_host', { args: { alias } });
  if (r.ok) await loadHosts();
  return r;
}

export async function hideHost(
  alias: string,
  hidden: boolean,
): Promise<Result<void>> {
  const r = await invokeCmd<void>('hide_host', { args: { alias, hidden } });
  if (r.ok) await loadHosts();
  return r;
}
