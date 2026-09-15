import { writable, derived } from 'svelte/store';
import { invokeCmd, invokeCmdAbortable, type Result } from './result';

export interface AccountRow {
  uuid: string;
  email: string | null;
  display_name: string | null;
  organization_name: string | null;
  organization_uuid: string | null;
  seat_tier: string | null;
  last_seen_at: number | null;
  nickname: string | null;
  has_extra_usage: boolean;
}

/**
 * The compact label for an account: its nickname when set, else its email,
 * else the first 8 characters of its uuid, else a fallback for "no account
 * at all" (a `null`/`undefined` row — e.g. a host with no linked account).
 */
export function accountLabel(a: AccountRow | null | undefined): string {
  if (!a) return 'unknown account';
  const nickname = a.nickname?.trim();
  if (nickname) return nickname;
  const email = a.email?.trim();
  if (email) return email;
  if (a.uuid) return a.uuid.slice(0, 8);
  return 'unknown account';
}

export const accounts = writable<AccountRow[]>([]);

/** "email (seat_tier)" — the one-line account label used in session
 *  details and the prompt composer; '—' when there is no account. */
export function accountEmailTier(a: AccountRow | null): string {
  if (!a) return '—';
  const email = a.email ?? a.uuid;
  return a.seat_tier ? `${email} (${a.seat_tier})` : email;
}

/** O(1) uuid -> account lookup, derived once per `accounts` change. */
export const accountByUuid = derived(accounts, ($a) => new Map($a.map((a) => [a.uuid, a])));

export async function loadAccounts(): Promise<Result<AccountRow[]>> {
  const r = await invokeCmd<AccountRow[]>('list_accounts');
  if (r.ok) accounts.set(r.value);
  return r;
}

function mergeInto(arr: AccountRow[], row: AccountRow): AccountRow[] {
  const i = arr.findIndex((a) => a.uuid === row.uuid);
  if (i === -1) return [...arr, row];
  const next = arr.slice();
  next[i] = row;
  return next;
}

function mergeAccount(row: AccountRow): void {
  accounts.update((arr) => mergeInto(arr, row));
}

/** Set (or, with `null`/empty/whitespace, clear) an account's nickname. */
export async function setAccountNickname(
  uuid: string,
  nickname: string | null,
): Promise<Result<AccountRow>> {
  const r = await invokeCmd<AccountRow>('set_account_nickname', { args: { uuid, nickname } });
  if (r.ok) mergeAccount(r.value);
  return r;
}

/** Apply a burst of `account:upserted` rows in ONE store update. */
export function applyAccountEvents(rows: readonly AccountRow[]): void {
  if (rows.length === 0) return;
  accounts.update((arr) => rows.reduce(mergeInto, arr));
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

export async function probeSshAliasAbortable(
  sshAlias: string,
  signal?: AbortSignal,
): Promise<Result<ProbePreview>> {
  return invokeCmdAbortable<ProbePreview>(
    'probe_ssh_alias',
    { args: { ssh_alias: sshAlias } },
    signal,
  );
}
