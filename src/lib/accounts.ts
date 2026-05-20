import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export interface AccountRow {
  uuid: string;
  email: string | null;
  display_name: string | null;
  organization_name: string | null;
  organization_uuid: string | null;
  seat_tier: string | null;
  last_seen_at: number | null;
}

export const accounts = writable<AccountRow[]>([]);

export async function loadAccounts(): Promise<Result<AccountRow[]>> {
  const r = await invokeCmd<AccountRow[]>('list_accounts');
  if (r.ok) accounts.set(r.value);
  return r;
}

export async function bootstrapAccounts(): Promise<void> {
  const r = await invokeCmd<AccountRow[]>('list_accounts');
  if (r.ok) accounts.set(r.value);
}

export function mergeAccount(row: AccountRow): void {
  accounts.update((arr) => {
    const i = arr.findIndex((a) => a.uuid === row.uuid);
    if (i === -1) return [...arr, row];
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

// No removeAccount — backend never deletes accounts in iter 4a.

// Used by AddHostPicker to preview probe results without persisting.
export interface ProbePreview {
  reachable: boolean;
  claude_version: string | null;
  tmux_version: string | null;
  account: {
    uuid: string | null;
    email: string | null;
    display_name: string | null;
    organization_name: string | null;
    organization_uuid: string | null;
    seat_tier: string | null;
  } | null;
}

export async function probeSshAlias(sshAlias: string): Promise<Result<ProbePreview>> {
  return invokeCmd<ProbePreview>('probe_ssh_alias', { args: { ssh_alias: sshAlias } });
}
