<script lang="ts">
  import type { Result } from './result';
  import {
    mcpConfigure,
    mcpClientConfig,
    installFleetHook,
    provisionHosts,
    type McpStatus,
    type HostProvisionResult,
  } from './mcp';

  let { onProvisioned }: { onProvisioned: () => Promise<void> } = $props();

  // --- Control API (MCP) ---
  let mcp: McpStatus | null = $state(null);
  let mcpBusy = $state(false);
  let mcpError: string | null = $state(null);
  let tokenShown = $state(false);
  // `bind:value` on a number input yields `null` when the field is cleared,
  // so the state is genuinely `number | null`.
  let portInput = $state<number | null>(4180);

  const configBlock = $derived(mcp ? mcpClientConfig(mcp) : '');

  // A valid TCP port: an integer in 1–65535. The Apply button and the
  // enable/regenerate paths refuse to forward anything outside this range.
  const portValid = $derived(
    portInput !== null &&
      Number.isInteger(portInput) &&
      portInput >= 1 &&
      portInput <= 65535,
  );
  // The port to send with a non-port change (toggle/regenerate): the typed
  // value when valid, else `undefined` so the backend keeps the current one.
  const safePort = $derived(portValid ? (portInput ?? undefined) : undefined);

  // The dialog fetches the status in its onMount (so the load order is
  // unchanged) and hands the result here.
  export function applyStatus(r: Result<McpStatus>) {
    if (r.ok && r.value) {
      mcp = r.value;
      portInput = r.value.port;
    } else if (!r.ok) {
      mcpError = r.error.message;
    }
  }

  async function applyMcp(opts: {
    enabled: boolean;
    port?: number;
    regenerateToken?: boolean;
    confirmDestructive?: boolean;
  }) {
    mcpBusy = true;
    mcpError = null;
    const r = await mcpConfigure(opts);
    mcpBusy = false;
    if (r.ok && r.value) {
      mcp = r.value;
      portInput = r.value.port;
    } else if (!r.ok) {
      mcpError = r.error.message;
    }
  }

  function maskToken(t: string): string {
    return t.length > 4 ? '••••••••••••' + t.slice(-4) : '••••';
  }

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      /* clipboard unavailable — no-op */
    }
  }

  // --- Install fleet hook ---
  let hookInstallMsg = $state<string | null>(null);
  let hookInstallError = $state<string | null>(null);
  let installingHook = $state(false);

  async function doInstallHook() {
    installingHook = true;
    hookInstallMsg = null;
    hookInstallError = null;
    try {
      hookInstallMsg = await installFleetHook('local');
    } catch (e: unknown) {
      const err = e as { message?: string };
      hookInstallError = err.message ?? String(e);
    } finally {
      installingHook = false;
    }
  }

  // --- Provision hosts ---
  let provisionResults = $state<HostProvisionResult[] | null>(null);
  let provisionBusy = $state(false);
  let provisionError: string | null = $state(null);

  async function doProvisionHosts(rotate = false) {
    provisionBusy = true;
    provisionError = null;
    provisionResults = null;
    const r = await provisionHosts(rotate);
    provisionBusy = false;
    if (r.ok && r.value) {
      provisionResults = r.value;
    } else if (!r.ok) {
      provisionError = r.error.message;
    }
    await onProvisioned();
  }
</script>

<section class="block" data-testid="mcp-section">
  <div class="section-header">
    <h4>Control API (MCP)</h4>
  </div>
  <p class="mcp-blurb">
    Lets an AI assistant drive claude-fleet over a localhost-only MCP
    server. Off by default. Every request needs the bearer token.
  </p>
  {#if mcp}
    <div class="mcp-row">
      <label class="toggle">
        <input
          type="checkbox"
          checked={mcp.enabled}
          disabled={mcpBusy}
          onchange={() => applyMcp({ enabled: !mcp!.enabled, port: safePort })}
          data-testid="mcp-enable" />
        Enable control API
      </label>
      <span class="status status-{mcp.running ? 'on' : 'off'}">
        {mcp.running ? 'running' : 'stopped'}
      </span>
    </div>
    {#if mcp.bind_error}
      <p class="err">Server could not start: {mcp.bind_error}</p>
    {/if}

    <div class="mcp-field">
      <span class="lbl">Port</span>
      <input
        class="port"
        class:invalid={!portValid}
        type="number"
        min="1"
        max="65535"
        bind:value={portInput}
        disabled={mcpBusy} />
      <button
        disabled={mcpBusy || !portValid || portInput === mcp.port}
        onclick={() => applyMcp({ enabled: mcp!.enabled, port: portInput ?? undefined })}>
        Apply
      </button>
      {#if !portValid}
        <span class="err">Port must be 1–65535.</span>
      {/if}
    </div>

    <div class="mcp-field">
      <span class="lbl">URL</span>
      <code class="mono">{mcp.url}</code>
      <button onclick={() => copyText(mcp!.url)}>Copy</button>
    </div>

    <div class="mcp-field">
      <span class="lbl">Token</span>
      <code class="mono token">{tokenShown ? mcp.token : maskToken(mcp.token)}</code>
      <button onclick={() => (tokenShown = !tokenShown)}>
        {tokenShown ? 'Hide' : 'Show'}
      </button>
      <button onclick={() => copyText(mcp!.token)}>Copy</button>
      <button
        class="danger"
        disabled={mcpBusy}
        onclick={() =>
          applyMcp({ enabled: mcp!.enabled, port: safePort, regenerateToken: true })}
        title="Mint a new token — invalidates existing clients">
        Regenerate
      </button>
    </div>

    <details class="mcp-config">
      <summary>MCP client config</summary>
      <pre>{configBlock}</pre>
      <button onclick={() => copyText(configBlock)}>Copy config</button>
    </details>

    <div class="mcp-row">
      <label class="toggle">
        <input
          type="checkbox"
          checked={mcp.confirm_destructive}
          disabled={mcpBusy}
          onchange={() =>
            applyMcp({
              enabled: mcp!.enabled,
              port: safePort,
              confirmDestructive: !mcp!.confirm_destructive,
            })}
          data-testid="mcp-confirm-destructive" />
        Ask me before agents broadcast, kill sessions, delete worktrees or write the clipboard
      </label>
    </div>

    <div class="hook-section">
      <p class="hook-desc">
        Install a real-time hook so local Claude Code sessions notify fleet
        immediately on stop or worktree creation.
      </p>
      <button
        class="hook-btn"
        onclick={doInstallHook}
        disabled={installingHook || !mcp.running}
        data-testid="install-fleet-hook"
      >
        {installingHook ? "Installing…" : "Install Hook (local)"}
      </button>
      {#if hookInstallMsg}
        <p class="hook-ok">{hookInstallMsg}</p>
      {/if}
      {#if hookInstallError}
        <p class="hook-err">{hookInstallError}</p>
      {/if}
    </div>

    <div class="hook-section">
      <p class="hook-desc">
        Push the MCP server config, a per-host bearer token and the
        Stop/WorktreeCreate hooks to every host so agents can connect to
        the control API. Existing tokens are reused; "Rotate all" mints
        fresh ones.
      </p>
      <div class="hook-actions">
        <button
          class="hook-btn"
          onclick={() => doProvisionHosts(false)}
          disabled={provisionBusy || !mcp.enabled}
          data-testid="provision-hosts"
        >
          {provisionBusy ? "Provisioning…" : "Provision hosts"}
        </button>
        <button
          class="hook-btn"
          onclick={() => doProvisionHosts(true)}
          disabled={provisionBusy || !mcp.enabled}
          title="Mint a fresh token for every host and re-provision"
          data-testid="provision-hosts-rotate"
        >
          Rotate all tokens
        </button>
      </div>
      {#if provisionError}
        <p class="hook-err">{provisionError}</p>
      {/if}
      {#if provisionResults}
        <table class="provision-table">
          <thead>
            <tr>
              <th>Host</th>
              <th>Status</th>
              <th>Detail</th>
            </tr>
          </thead>
          <tbody>
            {#each provisionResults as row (row.host)}
              <tr>
                <td class="alias">{row.host}</td>
                <td>
                  <span class="status status-{row.status === 'provisioned' ? 'on' : row.status === 'failed' ? 'off' : 'neutral'}">
                    {row.status}
                  </span>
                </td>
                <td class="provision-detail">{row.detail ?? '—'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        <p class="hook-desc provision-note">
          Restart Claude on each host to load the server (the skill is picked up live).
        </p>
      {/if}
    </div>
  {/if}
  {#if mcpError}<p class="err">{mcpError}</p>{/if}
</section>

<style>
  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.4rem;
  }
  .section-header h4 {
    margin: 0;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .alias { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }

  .status {
    font-size: 0.7rem;
    padding: 0.1rem 0.45rem;
    border-radius: 999px;
  }
  .status-on { background: rgba(60,180,90,0.18); color: rgb(80,200,110); }
  .status-off { background: rgba(180,100,100,0.18); color: rgb(220,130,130); }
  .hook-actions { display: flex; gap: 0.4rem; flex-wrap: wrap; }

  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }

  .mcp-blurb {
    font-size: 0.78rem;
    color: var(--fg-muted);
    margin: 0 0 0.6rem;
    line-height: 1.4;
  }
  .mcp-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.5rem;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .mcp-field {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin-bottom: 0.4rem;
    font-size: 0.82rem;
  }
  .mcp-field .lbl {
    width: 3.2rem;
    color: var(--fg-muted);
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .mcp-field .mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    background: var(--bg-alt, rgba(127, 127, 127, 0.12));
    padding: 0.1rem 0.4rem;
    border-radius: 3px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mcp-field .token {
    flex: 1;
    min-width: 0;
  }
  .mcp-field .port {
    width: 6rem;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: 4px;
    padding: 0.2rem 0.4rem;
  }
  .mcp-field .port.invalid {
    border-color: #e64a4a;
  }
  .mcp-field button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    cursor: pointer;
    padding: 0.18rem 0.5rem;
    font-size: 0.78rem;
    border-radius: 4px;
  }
  .mcp-field button:hover:not(:disabled) { border-color: var(--accent); }
  .mcp-field button:disabled { opacity: 0.5; cursor: default; }
  .mcp-field button.danger:hover:not(:disabled) {
    color: #e64a4a;
    border-color: #e64a4a;
  }
  .mcp-config { font-size: 0.8rem; margin-top: 0.3rem; }
  .mcp-config summary { cursor: pointer; color: var(--fg-muted); }
  .mcp-config pre {
    background: var(--bg-alt, rgba(127, 127, 127, 0.12));
    padding: 0.5rem;
    border-radius: 4px;
    overflow: auto;
    font-size: 0.75rem;
    margin: 0.4rem 0;
  }
  .mcp-config button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    cursor: pointer;
    padding: 0.18rem 0.5rem;
    font-size: 0.78rem;
    border-radius: 4px;
  }
  .mcp-config button:hover { border-color: var(--accent); }

  .hook-section {
    margin-top: 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .hook-desc {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary, #888);
  }
  .hook-btn {
    align-self: flex-start;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    cursor: pointer;
    padding: 0.18rem 0.5rem;
    font-size: 0.78rem;
    border-radius: 4px;
  }
  .hook-btn:hover:not(:disabled) { border-color: var(--accent); }
  .hook-btn:disabled { opacity: 0.5; cursor: default; }
  .hook-ok {
    margin: 0;
    font-size: 12px;
    color: var(--color-success, #4caf50);
    white-space: pre-wrap;
  }
  .hook-err {
    margin: 0;
    font-size: 12px;
    color: var(--color-error, #f44336);
  }

  .provision-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.82rem;
    margin-top: 0.3rem;
  }
  .provision-table th {
    text-align: left;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
    padding: 0.3rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
  .provision-table td {
    padding: 0.35rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
  .provision-detail {
    color: var(--fg-muted);
    font-size: 0.78rem;
  }
  .status-neutral {
    background: rgba(127, 127, 127, 0.15);
    color: var(--fg-muted);
  }
  .provision-note {
    margin-top: 0.4rem;
    font-style: italic;
  }
</style>
