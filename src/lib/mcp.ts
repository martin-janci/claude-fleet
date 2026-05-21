import { invokeCmd, type Result } from './result';

/// Status of the embedded MCP control-API server. Mirrors the backend
/// `McpStatus` struct (`commands/mcp.rs`).
export interface McpStatus {
  /** Persisted on/off setting — may be true even when `running` is false. */
  enabled: boolean;
  /** Whether the server is actually listening right now. */
  running: boolean;
  port: number;
  /** Bearer token clients must present. */
  token: string;
  /** Full streamable-HTTP endpoint URL. */
  url: string;
  /** Most recent start failure (e.g. port in use), or null. */
  bind_error: string | null;
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
    },
  });
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
