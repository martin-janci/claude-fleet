<script lang="ts">
  // The states kit's offline host: said inside the host's own group or pane,
  // never as a window-wide error. What happened, how long ago, what it means
  // for the sessions there, and the next step.
  import { sinceWords } from './states';

  let {
    alias,
    lastSeen = null,
    now = Math.floor(Date.now() / 1000),
    reason = null,
    code = null,
    sessions = null,
    ontry = null,
    trying = false,
    tryBlocked = null,
    onopen = null,
  }: {
    alias: string;
    /** When the host last answered (seconds since the epoch). */
    lastSeen?: number | null;
    now?: number;
    /** Why the last probe failed, when known ("SSH timed out after 10 s"). */
    reason?: string | null;
    /** Its error code, shown under Details. */
    code?: string | null;
    /** How many sessions live there. */
    sessions?: number | null;
    ontry?: (() => void) | null;
    trying?: boolean;
    /** Why Try again cannot run here (a hub client without the hub). */
    tryBlocked?: string | null;
    onopen?: (() => void) | null;
  } = $props();

  const since = $derived(sinceWords(lastSeen, now));
</script>

<div class="host-offline" role="status" data-testid="host-offline-state" data-alias={alias}>
  <p class="head">
    <span class="dot" aria-hidden="true">○</span>
    <strong>{alias}</strong> is offline{#if since}<span class="muted"> · last answered {since} ago</span>{/if}
  </p>
  <p class="body">
    {#if reason}{reason}. {/if}{#if sessions}Its {sessions === 1 ? 'session is' : `${sessions} sessions are`} probably still running in tmux and reattach when the host is back.{:else}Sessions there reattach when the host is back.{/if}
  </p>
  {#if code}
    <details class="details">
      <summary>Details</summary>
      <code data-testid="host-offline-code">{code}</code>
    </details>
  {/if}
  {#if ontry || onopen}
    <div class="actions">
      {#if ontry}
        <button
          type="button"
          class="btn"
          disabled={trying || tryBlocked !== null}
          title={tryBlocked ?? ''}
          data-testid="host-offline-try"
          onclick={ontry}>{trying ? 'Trying…' : 'Try again'}</button>
      {/if}
      {#if onopen}
        <button type="button" class="btn" data-testid="host-offline-open" onclick={onopen}>Host detail</button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .host-offline {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding: 0.55rem 0.7rem;
    border: 1px solid color-mix(in srgb, var(--usage-warn) 45%, transparent);
    border-radius: 6px;
    background: color-mix(in srgb, var(--usage-warn) 7%, transparent);
    font-size: 0.82rem;
  }
  .head, .body {
    margin: 0;
  }
  .dot {
    color: var(--usage-warn);
  }
  .muted, .body {
    color: var(--fg-muted);
  }
  .details summary {
    cursor: pointer;
    font-size: 11px;
    color: var(--fg-muted);
  }
  .details code {
    font-size: 11px;
  }
  .actions {
    display: flex;
    gap: 0.4rem;
  }
</style>
