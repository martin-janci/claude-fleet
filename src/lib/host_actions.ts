// Host actions for every surface that manages hosts (the Hosts view, and
// Settings' Control-API section, which refreshes the token cache after
// provisioning): the per-host control-API token state (list, mode, rotate),
// re-probe, and hide with Undo. Store mutations stay in `hosts.ts` / `mcp.ts`;
// this module owns the shared token cache and the user-facing toasts, so no
// logic exists twice.
import { writable } from 'svelte/store';
import { hideHost, probeHost, type HostRow } from './hosts';
import {
  listHostTokens,
  rotateHostToken,
  setHostTokenMode,
  type HostTokenInfo,
  type TokenMode,
} from './mcp';
import type { Result } from './result';
import { push, pushError } from './toasts';

/** alias → token info; a host absent here has never been provisioned. */
export const hostTokens = writable<Map<string, HostTokenInfo>>(new Map());
/** False until the first successful `list_host_tokens`. */
export const hostTokensLoaded = writable(false);

export async function loadHostTokens(): Promise<void> {
  const r = await listHostTokens();
  if (r.ok && Array.isArray(r.value)) {
    hostTokens.set(new Map(r.value.map((t) => [t.host_alias, t])));
    hostTokensLoaded.set(true);
  }
}

function patchToken(alias: string, r: Result<HostTokenInfo>): void {
  if (r.ok && r.value) {
    const info = r.value;
    hostTokens.update((m) => new Map(m).set(alias, info));
  }
}

export async function setTokenMode(alias: string, mode: TokenMode): Promise<Result<HostTokenInfo>> {
  const r = await setHostTokenMode(alias, mode);
  patchToken(alias, r);
  return r;
}

/** Mint a fresh token for `alias` and re-provision the host with it. */
export async function rotateToken(alias: string): Promise<Result<HostTokenInfo>> {
  const r = await rotateHostToken(alias);
  patchToken(alias, r);
  return r;
}

/** Re-probe with the failure surfaced as a toast. */
export async function reprobeHost(alias: string): Promise<Result<HostRow>> {
  const r = await probeHost(alias);
  if (!r.ok) pushError(r.error, `Re-probe ${alias} failed`);
  return r;
}

/** Show a hidden host again (no confirm, no toast: nothing to undo). */
export async function showHost(alias: string): Promise<Result<HostRow>> {
  const r = await hideHost(alias, false);
  if (!r.ok) pushError(r.error, `Show ${alias} failed`);
  return r;
}

/** Hide immediately, with an `Undo` toast that shows the host again. */
export async function hideHostWithUndo(alias: string): Promise<Result<HostRow>> {
  const r = await hideHost(alias, true);
  if (!r.ok) {
    pushError(r.error, `Hide ${alias} failed`);
    return r;
  }
  push({
    kind: 'info',
    message: `${alias} is hidden from the sidebar.`,
    action: { label: 'Undo', run: () => void showHost(alias) },
  });
  return r;
}
