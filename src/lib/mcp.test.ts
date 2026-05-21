import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { mcpStatus, mcpConfigure, mcpClientConfig, type McpStatus } from './mcp';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const sample: McpStatus = {
  enabled: true,
  running: true,
  port: 4180,
  token: 'abcd1234',
  url: 'http://127.0.0.1:4180/mcp',
  bind_error: null,
};

beforeEach(() => {
  inv.mockReset();
});

describe('mcp store', () => {
  it('mcpStatus invokes mcp_status and returns the status', async () => {
    inv.mockResolvedValueOnce(sample);
    const r = await mcpStatus();
    expect(r.ok).toBe(true);
    expect(r.ok && r.value.port).toBe(4180);
    expect(inv).toHaveBeenCalledWith('mcp_status', undefined);
  });

  it('mcpConfigure wraps args with snake_case and defaults', async () => {
    inv.mockResolvedValueOnce(sample);
    await mcpConfigure({ enabled: true });
    expect(inv).toHaveBeenCalledWith('mcp_configure', {
      args: { enabled: true, port: null, regenerate_token: false },
    });
  });

  it('mcpConfigure forwards port and regenerateToken', async () => {
    inv.mockResolvedValueOnce(sample);
    await mcpConfigure({ enabled: false, port: 5000, regenerateToken: true });
    expect(inv).toHaveBeenCalledWith('mcp_configure', {
      args: { enabled: false, port: 5000, regenerate_token: true },
    });
  });

  it('mcpClientConfig builds a streamable-HTTP config with the bearer header', () => {
    const cfg = JSON.parse(mcpClientConfig(sample));
    expect(cfg.mcpServers['claude-fleet'].url).toBe('http://127.0.0.1:4180/mcp');
    expect(cfg.mcpServers['claude-fleet'].type).toBe('http');
    expect(cfg.mcpServers['claude-fleet'].headers.Authorization).toBe(
      'Bearer abcd1234',
    );
  });
});
