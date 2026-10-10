<script lang="ts">
  // Open offline (redesign step 3.15, offline.ts): this computer's sessions
  // while the hub cannot be reached, and nothing of the hub's fleet. A layer
  // over the window, like the splash it replaces; it steps aside when the hub
  // answers.
  import { onMount } from 'svelte';
  import Loader from './Loader.svelte';
  import EmptyState from './states/EmptyState.svelte';
  import { hubStatus } from './hub';
  import { retryHubNow } from './hub_connection';
  import { loadOfflineSessions, OFFLINE_HOST, offlineSessions, type OfflineSession } from './offline';
  import { openSessionInEditor } from './editor';
  import { copyText } from './clipboard';
  import { sinceWords } from './states/states';
  import { push } from './toasts';

  let { onhubsettings }: { onhubsettings: () => void } = $props();

  let loading = $state(true);
  let error = $state<string | null>(null);
  let now = $state(Math.floor(Date.now() / 1000));
  const hubName = $derived($hubStatus.url ?? $hubStatus.configured_url ?? 'the hub');

  async function load() {
    loading = true;
    const r = await loadOfflineSessions();
    error = r.ok ? null : r.error.message;
    now = Math.floor(Date.now() / 1000);
    loading = false;
  }
  onMount(() => void load());

  async function copyAttach(s: OfflineSession) {
    if (await copyText(s.attach)) push({ kind: 'info', message: `Copied: ${s.attach}` });
  }
</script>

<div class="offline" data-testid="offline-fleet" role="region" aria-label="Offline: this computer's sessions">
  <div class="head">
    <Loader name="signal-lost" size={20} delay={0} testid="offline-mark" />
    <div class="what">
      <p class="title">Offline</p>
      <p class="detail">Cannot reach {hubName}. Only this computer's sessions are listed; the hub's hosts keep running.</p>
    </div>
    <div class="actions">
      <button type="button" class="btn btn--primary" data-testid="offline-retry" onclick={() => void retryHubNow()}>Retry</button>
      <button type="button" class="btn" data-testid="offline-hub-settings" onclick={onhubsettings}>Hub settings…</button>
    </div>
  </div>

  <section class="host" data-testid="offline-host" data-alias={OFFLINE_HOST} aria-label="Sessions on this computer">
    <h2 class="host-name">This computer <span class="muted">{OFFLINE_HOST}</span></h2>
    {#if loading}
      <Loader name="comet" size={16} label="Loading" testid="offline-loading" />
    {:else if error}
      <p class="error" data-testid="offline-error">{error}</p>
      <div><button type="button" class="btn" onclick={() => void load()}>Try again</button></div>
    {:else if $offlineSessions.length === 0}
      <EmptyState kind="calm" testid="offline-empty" title="No sessions on this computer" body="Sessions on your other hosts are with the hub." />
    {:else}
      <ul class="sessions">
        {#each $offlineSessions as s (s.name)}
          {@const since = sinceWords(s.last_activity, now)}
          <li class="session" data-testid="offline-session">
            <span class="name">{s.name}</span>
            <span class="muted">{s.attached ? 'Attached' : 'Idle'}{#if since} · {since} ago{/if}</span>
            <span class="row-actions">
              <button type="button" class="btn" data-testid="offline-copy" onclick={() => void copyAttach(s)}>Copy attach command</button>
              <button
                type="button"
                class="btn"
                data-testid="offline-editor"
                onclick={() => void openSessionInEditor({ host_alias: OFFLINE_HOST, tmux_name: s.name, kind: 'claude' })}>Open in VS Code</button>
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</div>

<style>
  .offline {
    position: fixed;
    inset: 0;
    z-index: 900;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: var(--space-6);
    overflow: auto;
    background: var(--bg);
    color: var(--fg);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    flex-wrap: wrap;
  }
  .what {
    flex: 1 1 auto;
    min-width: 0;
  }
  .title {
    margin: 0;
    font-size: var(--text-md);
    font-weight: 600;
  }
  .detail,
  .muted {
    margin: 0;
    font-size: var(--text-sm);
    color: var(--fg-muted);
  }
  .actions,
  .row-actions {
    display: flex;
    gap: var(--space-2);
  }
  .host {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: var(--prose-max);
  }
  .host-name {
    margin: 0;
    font-size: var(--text-sm);
    font-weight: 600;
  }
  .sessions {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .session {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
  }
  .name {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row-actions {
    margin-left: auto;
  }
  .error {
    margin: 0;
    color: var(--status-failed);
    font-size: var(--text-sm);
  }
</style>
