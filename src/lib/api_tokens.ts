// Named Control API tokens (M15 step G2.8): Settings › Control API's
// + Token, through the `api_tokens` command (the desktop's own control API,
// or the hub's when paired). The token itself exists only in `create`'s
// answer: it is shown once (ApiTokenCreated) and never kept here.
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import type { Values } from './forms/forms';

export type ApiScope = 'read' | 'act' | 'admin';

export interface ApiTokenRow {
  id: number;
  name: string;
  scope: ApiScope;
  /** The hosts it reaches; null is every host. */
  hosts: string[] | null;
  expires_at: number | null;
  created_at: number;
  last_used_at: number | null;
  revoked_at: number | null;
}

export interface ApiTokenCreated extends ApiTokenRow {
  token: string;
  /** `FLEET_MCP_TOKEN=<token>`, for Copy as env line. */
  env_line: string;
}

export const apiTokens = writable<ApiTokenRow[]>([]);

export async function loadApiTokens(): Promise<void> {
  const r = await invokeCmd<ApiTokenRow[]>('api_tokens', { args: { action: 'list' } });
  if (r.ok && Array.isArray(r.value)) apiTokens.set(r.value);
}

/** `api_tokens create`'s arguments from the New token form's answers. */
export function createTokenArgs(v: Values): Record<string, unknown> {
  const scope: ApiScope = v.scope === 'read' || v.scope === 'admin' ? v.scope : 'act';
  const days = typeof v.expires === 'string' && /^\d+$/.test(v.expires) ? Number(v.expires) : null;
  const hosts = Array.isArray(v.hosts) ? v.hosts.map(String).filter((h) => h.trim() !== '') : [];
  return {
    action: 'create',
    name: String(v.name ?? '').trim(),
    scope,
    expires_in_days: days,
    // None picked is every host; an admin token is never limited.
    hosts: scope === 'admin' || hosts.length === 0 ? null : hosts,
  };
}

export async function createApiToken(v: Values): Promise<Result<ApiTokenCreated>> {
  const r = await invokeCmd<ApiTokenCreated>('api_tokens', { args: createTokenArgs(v) });
  if (r.ok) await loadApiTokens();
  return r;
}

export async function revokeApiToken(name: string): Promise<Result<ApiTokenRow>> {
  const r = await invokeCmd<ApiTokenRow>('api_tokens', { args: { action: 'revoke', name } });
  await loadApiTokens();
  return r;
}


/** "expires in 12 d", "expired", or "" for a token that never expires. */
export function expiryLabel(expiresAt: number | null, now: number): string {
  if (expiresAt === null) return '';
  const left = expiresAt - now;
  if (left <= 0) return 'expired';
  const days = Math.floor(left / 86_400);
  if (days >= 1) return `expires in ${days} d`;
  const hours = Math.max(1, Math.floor(left / 3600));
  return `expires in ${hours} h`;
}
