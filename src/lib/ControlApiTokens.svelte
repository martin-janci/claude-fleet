<script lang="ts">
  // Settings → Control API: every token that reaches this fleet's control
  // API in one table (Orbit Fleet 11.4) — each host's own token, with when
  // it was last used and last rotated, beside the paired devices' tokens.
  // A host's token is rotated here (`rotate_host_token`, which re-provisions
  // the host); a device's is revoked (`revoke_device`, as Settings → Devices
  // does). The master token stays in the section above.
  //
  // M15 step G2.8: + Token creates a named token (read, act or admin, an
  // expiry, a host limit) through the New Control API token form; it is
  // shown once (ApiTokenCreated) and listed here by name after, with Revoke.
  import ConfirmDialog from './ConfirmDialog.svelte';
  import WizardDialog from './forms/WizardDialog.svelte';
  import ApiTokenCreated from './ApiTokenCreated.svelte';
  import { WIZARDS, withChoices } from './forms/wizards';
  import {
    apiTokens,
    loadApiTokens,
    createApiToken,
    revokeApiToken,
    expiryLabel,
    type ApiTokenCreated as Created,
  } from './api_tokens';
  import { hosts } from './hosts';
  import type { IpcError } from './result';
  import { hostTokens, loadHostTokens, rotateToken } from './host_actions';
  import { rotateTokenMessage } from './hosts_view';
  import { devices, loadDevices, type DeviceSummary } from './devices';
  import { invokeCmd } from './result';
  import { push, pushError } from './toasts';
  import { ago } from './pages/resources';

  let {
    active = true,
    now = () => Math.floor(Date.now() / 1000),
  }: {
    /** Read the tokens once this is true: Settings keeps every panel
     *  mounted, and a hidden one asks nothing. */
    active?: boolean;
    now?: () => number;
  } = $props();

  type Row =
    | { kind: 'host'; key: string; name: string; mode: string; used?: number; created: number; rotated?: number }
    | { kind: 'device'; key: string; name: string; mode: string; used?: number; created: number; self: boolean }
    | { kind: 'named'; key: string; name: string; mode: string; used?: number; created: number; hosts: string[] | null; expires: number | null };

  const rows = $derived<Row[]>([
    ...$apiTokens.map(
      (t): Row => ({
        kind: 'named',
        key: `named:${t.name}`,
        name: t.name,
        mode: t.scope,
        used: t.last_used_at ?? undefined,
        created: t.created_at,
        hosts: t.hosts,
        expires: t.expires_at,
      }),
    ),
    ...[...$hostTokens.values()].map(
      (t): Row => ({
        kind: 'host',
        key: `host:${t.host_alias}`,
        name: t.host_alias,
        mode: t.mode,
        used: t.last_used_at ?? undefined,
        created: t.created_at,
        rotated: t.rotated_at ?? undefined,
      }),
    ),
    ...$devices.map(
      (d: DeviceSummary): Row => ({
        kind: 'device',
        key: `device:${d.name}`,
        name: d.name,
        mode: d.mode,
        used: d.last_seen_at,
        created: d.created_at,
        self: d.this_device === true,
      }),
    ),
  ]);

  let asking = $state<Row | null>(null);
  let busy = $state(false);

  let creating = $state(false);
  let createBusy = $state(false);
  let createError = $state<IpcError | null>(null);
  let created = $state<Created | null>(null);
  const tokenWizard = $derived({
    ...WIZARDS.new_token,
    spec: withChoices(WIZARDS.new_token.spec, {
      hosts: $hosts.filter((h) => !h.hidden).map((h): [string, string] => [h.alias, h.alias]),
    }),
  });

  async function create(v: import('./forms/forms').Values) {
    createBusy = true;
    createError = null;
    const r = await createApiToken(v);
    createBusy = false;
    if (r.ok) {
      creating = false;
      created = r.value;
    } else createError = r.error;
  }

  let loaded = false;
  $effect(() => {
    if (active && !loaded) {
      loaded = true;
      void loadHostTokens();
      void loadDevices();
      void loadApiTokens();
    }
  });

  async function go(row: Row) {
    busy = true;
    if (row.kind === 'named') {
      const r = await revokeApiToken(row.name);
      if (r.ok) push({ kind: 'success', message: `${row.name} can no longer reach the fleet.` });
      else pushError(r.error, `Revoke ${row.name} failed`);
    } else if (row.kind === 'host') {
      const r = await rotateToken(row.name);
      if (r.ok) {
        push({ kind: 'success', message: `${row.name} has a new control-API token.` });
        if (r.value.warning) push({ kind: 'warning', message: `${row.name}: ${r.value.warning}`, sticky: true });
      } else pushError(r.error, `Rotate token for ${row.name} failed`);
    } else {
      const r = await invokeCmd<unknown>('revoke_device', { args: { device: row.name } });
      if (r.ok) push({ kind: 'success', message: `${row.name} can no longer reach the fleet.` });
      else pushError(r.error, `Revoke ${row.name} failed`);
      await loadDevices();
    }
    busy = false;
    asking = null;
  }
</script>

<section class="block" data-testid="control-api-tokens">
  <div class="section-header">
    <h4>Control API tokens</h4>
    <button
      type="button"
      class="hook-btn"
      data-testid="token-new"
      onclick={() => {
        createError = null;
        creating = true;
      }}>+ Token</button
    >
  </div>
  {#if rows.length === 0}
    <p class="hook-desc" data-testid="tokens-empty">No token yet. Provision a host, pair a device, or create one with + Token.</p>
  {:else}
    <table class="tokens">
      <thead>
        <tr><th>Token</th><th>Scope</th><th>Used</th><th>Created</th><th><span class="sr">Action</span></th></tr>
      </thead>
      <tbody>
        {#each rows as row (row.key)}
          <tr data-testid={`token-row-${row.key}`}>
            <td class="name">
              {row.name}
              <span class="kind">· {row.kind === 'host' ? 'per-host' : row.kind === 'device' ? 'device' : 'named'}</span>
              {#if row.kind === 'named' && (row.hosts || row.expires !== null)}
                <span class="rotated" data-testid={`token-limits-${row.key}`}
                  >{[row.hosts ? `only ${row.hosts.join(', ')}` : '', expiryLabel(row.expires, now())].filter(Boolean).join(' · ')}</span
                >
              {/if}
            </td>
            <td>{row.mode === 'readonly' ? 'read-only' : row.mode}</td>
            <td data-testid={`token-used-${row.key}`}>{ago(row.used, now())}</td>
            <td data-testid={`token-created-${row.key}`}>
              {ago(row.created, now())}
              {#if row.kind === 'host' && row.rotated}
                <span class="rotated" data-testid={`token-rotated-${row.key}`}>rotated {ago(row.rotated, now())}</span>
              {/if}
            </td>
            <td class="act">
              {#if row.kind === 'host'}
                <button type="button" class="hook-btn" disabled={busy} data-testid={`token-rotate-${row.name}`} onclick={() => (asking = row)}
                  >Rotate…</button
                >
              {:else if row.kind === 'named'}
                <button type="button" class="hook-btn" disabled={busy} data-testid={`token-revoke-named-${row.name}`} onclick={() => (asking = row)}
                  >Revoke…</button
                >
              {:else if !row.self}
                <button type="button" class="hook-btn" disabled={busy} data-testid={`token-revoke-${row.name}`} onclick={() => (asking = row)}
                  >Revoke…</button
                >
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    <p class="hook-desc">
      A host's token lives in its ~/.claude.json and is replaced by Rotate; a device's was shown once at pairing, and a
      named one at + Token, and both are ended by Revoke. Used is the last request the token made, to the minute.
    </p>
  {/if}
</section>

{#if creating}
  <WizardDialog
    wizard={tokenWizard}
    busy={createBusy}
    error={createError}
    errorTestid="token-new-error"
    run={(v) => void create(v)}
    onclose={() => (creating = false)} />
{/if}

{#if created}
  <ApiTokenCreated {created} onclose={() => (created = null)} />
{/if}

{#if asking}
  <ConfirmDialog
    title={asking.kind === 'host' ? `Rotate the token for ${asking.name}` : `Revoke ${asking.name}`}
    message={asking.kind === 'host'
      ? rotateTokenMessage(asking.name)
      : asking.kind === 'named'
        ? `Anything using ${asking.name} is refused from its next request. Create a new token to bring it back.`
        : `${asking.name} can no longer reach the fleet. Pair it again to bring it back.`}
    confirmLabel={asking.kind === 'host' ? 'Rotate' : 'Revoke'}
    danger={asking.kind !== 'host'}
    {busy}
    confirmTestId="token-confirm"
    onconfirm={() => asking && void go(asking)}
    oncancel={() => (asking = null)} />
{/if}

<style>
  .tokens {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-2xs);
  }
  .tokens th {
    text-align: left;
    font-weight: 500;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    padding: 0.25rem 0.5rem 0.25rem 0;
    border-bottom: 1px solid var(--border);
  }
  .tokens td {
    padding: 0.35rem 0.5rem 0.35rem 0;
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }
  .name {
    font-weight: 500;
  }
  .kind,
  .rotated {
    color: var(--fg-muted);
    font-weight: 400;
  }
  .rotated {
    display: block;
    font-size: var(--text-2xs);
  }
  .act {
    text-align: right;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
