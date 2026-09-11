import { invokeCmd, type Result } from './result';

/// Status of the embedded MCP control-API server. Mirrors the backend
/// `McpStatus` struct (`commands/mcp.rs`).
export interface McpStatus {
  /** Persisted on/off setting — may be true even when `running` is false. */
  enabled: boolean;
  /** Whether the server is actually listening right now. */
  running: boolean;
  port: number;
  /** Master bearer token (desktop / local clients). Hosts get their own. */
  token: string;
  /** Full streamable-HTTP endpoint URL. */
  url: string;
  /** Most recent start failure (e.g. port in use), or null. */
  bind_error: string | null;
  /** `mcp.confirm_destructive`: broadcast / kill / delete_worktree /
   *  set_clipboard need a desktop confirmation. Default off. */
  confirm_destructive: boolean;
}

/** Read the current control-API status. */
export async function mcpStatus(): Promise<Result<McpStatus>> {
  return invokeCmd<McpStatus>('mcp_status');
}

/**
 * Apply a control-API configuration change. The backend persists the settings
 * and starts/stops the server live. Returns the resulting status.
 */
export async function mcpConfigure(opts: {
  enabled: boolean;
  port?: number;
  regenerateToken?: boolean;
  confirmDestructive?: boolean;
}): Promise<Result<McpStatus>> {
  // `?? null` is not enough — `NaN ?? null` is `NaN`. Only forward a real,
  // integral port; anything else is sent as null so the backend keeps the
  // current one.
  const port =
    typeof opts.port === 'number' && Number.isInteger(opts.port) ? opts.port : null;
  return invokeCmd<McpStatus>('mcp_configure', {
    args: {
      enabled: opts.enabled,
      port,
      regenerate_token: opts.regenerateToken ?? false,
      confirm_destructive: opts.confirmDestructive ?? null,
    },
  });
}

/** Install the fleet hook on the given host so it notifies fleet on session events. */
export async function installFleetHook(hostAlias: string): Promise<string> {
  const r = await invokeCmd<string>('install_fleet_hook', { hostAlias });
  if (r.ok) return r.value;
  throw r.error;
}

export interface HostProvisionResult {
  host: string;
  status: string; // provisioned | skipped | failed
  detail: string | null;
}

/** Provision every reachable host. `rotate` mints fresh per-host tokens. */
export function provisionHosts(rotate = false): Promise<Result<HostProvisionResult[]>> {
  return invokeCmd<HostProvisionResult[]>('provision_hosts', { rotate });
}

// --- per-host tokens ---------------------------------------------------------

/** A host's control-API token row, minus the token itself. Mirrors the
 *  backend `HostTokenInfo`. */
export interface HostTokenInfo {
  host_alias: string;
  /** `full` | `readonly` */
  mode: string;
  created_at: number;
}

export type TokenMode = 'full' | 'readonly';

export function listHostTokens(): Promise<Result<HostTokenInfo[]>> {
  return invokeCmd<HostTokenInfo[]>('list_host_tokens');
}

export function setHostTokenMode(
  hostAlias: string,
  mode: TokenMode,
): Promise<Result<HostTokenInfo>> {
  return invokeCmd<HostTokenInfo>('set_host_token_mode', { hostAlias, mode });
}

/** Mint a fresh token for one host and re-provision it with it. */
export function rotateHostToken(hostAlias: string): Promise<Result<HostTokenInfo>> {
  return invokeCmd<HostTokenInfo>('rotate_host_token', { hostAlias });
}

// --- destructive-call confirmation -------------------------------------------

/** Payload of the `mcp:confirm-required` event. Mirrors `guard::ConfirmRequest`. */
export interface ConfirmRequest {
  nonce: string;
  tool: string;
  /** Redacted argument summary (never a prompt body). */
  summary: string;
  /** `master` or `host:<alias>` */
  caller: string;
}

export const MCP_CONFIRM_EVENT = 'mcp:confirm-required';

/** Answer a confirmation prompt. Resolves `false` when the nonce expired. */
export function mcpConfirm(nonce: string, approved: boolean): Promise<Result<boolean>> {
  return invokeCmd<boolean>('mcp_confirm', { nonce, approved });
}

export function mcpPendingConfirms(): Promise<Result<{ nonce: string; tool: string }[]>> {
  return invokeCmd<{ nonce: string; tool: string }[]>('mcp_pending_confirms');
}

/** Build the ready-to-paste MCP client config for an HTTP transport. */
export function mcpClientConfig(status: McpStatus): string {
  return JSON.stringify(
    {
      mcpServers: {
        'claude-fleet': {
          type: 'http',
          url: status.url,
          headers: { Authorization: `Bearer ${status.token}` },
        },
      },
    },
    null,
    2,
  );
}
