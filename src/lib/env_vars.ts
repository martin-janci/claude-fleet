// Session environment variables (spec 2026-10-11-session-env-and-secrets-design.md):
// Settings › Environment's list and actions, through the routed `env_vars`
// command (the desktop's own store, or the hub's tool when paired). A secret
// or a reference is write-only: a row carries `value` for a plain entry only.
import { invokeCmd, type Result } from './result';

/** global < host < project < person < session (the narrower wins). */
export type EnvScope = 'global' | 'host' | 'project' | 'person' | 'session';

export interface EnvVarRow {
  id: number;
  scope: EnvScope;
  /** '' for global, a host alias, owner/repo, a person id, or a session id. */
  scope_key: string;
  name: string;
  /** Present for a plain, non-reference entry only. */
  value?: string;
  secret: boolean;
  is_ref: boolean;
  updated_at: number;
}

export interface EnvVarsArgs {
  action: 'list' | 'set' | 'unset';
  scope?: EnvScope;
  /** For scope person: '' or 'me' (the caller's own person; nobody else's). */
  scope_key?: string;
  name?: string;
  value?: string;
  /** env:NAME or file:/abs, resolved at launch; the master's only. */
  ref?: string;
  secret?: boolean;
}

export const envVars = <T = unknown>(args: EnvVarsArgs): Promise<Result<T>> =>
  invokeCmd<T>('env_vars', { args });

export const listEnvVars = (): Promise<Result<EnvVarRow[]>> => invokeCmd<EnvVarRow[]>('env_vars_list');
