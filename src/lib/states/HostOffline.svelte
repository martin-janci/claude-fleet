<script lang="ts">
  // The states kit's offline host: said inside the host's own group or pane,
  // never as a window-wide error. What happened, how long ago, what it means
  // for the sessions there, and the next step.
  //
  // Step 3.14: the sessions it holds are named as Paused (they keep running in
  // tmux and reattach), with Show sessions to go to them, and Wake host only
  // where the host can be woken: the caller passes `onwake` for such a host,
  // and nothing in the fleet can wake one yet (no wake-on-LAN or other wake
  // route is configured anywhere), so no caller does today.
  import { sinceWords } from './states';
  import Loader from '../Loader.svelte';

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
    paused = [],
    onshow = null,
    onwake = null,
    waking = false,
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
    /** The names of the sessions there, which wait as Paused. */
    paused?: readonly string[];
    /** Show sessions: go to them (the host's session list). */
    onshow?: (() => void) | null;
    /** Wake host, only for a host that can be woken. */
    onwake?: (() => void) | null;
    waking?: boolean;
  } = $props();

  const since = $derived(sinceWords(lastSeen, now));
  /** The list names a few; the rest are counted. */
  const PAUSED_SHOWN = 4;
  const pausedShown = $derived(paused.slice(0, PAUSED_SHOWN));
  const pausedMore = $derived(Math.max(0, paused.length - PAUSED_SHOWN));
</script>

<div class="host-offline" role="status" data-testid="host-offline-state" data-alias={alias}>
  <p class="head">
    <!-- Redesign step 3.14: the kit's Signal lost, which drifts once and
         rests (loader-kit.css): an offline host is not something to wait on. -->
    <Loader name="signal-lost" size={20} delay={0} testid="host-offline-mark" />
    <span><strong>{alias}</strong> is offline{#if since}<span class="muted"> · last answered {since} ago</span>{/if}</span>
  </p>
  <p class="body">
    {#if reason}{reason}. {/if}{#if sessions}Its {sessions === 1 ? 'session is' : `${sessions} sessions are`} probably still running in tmux and reattach when the host is back.{:else}Sessions there reattach when the host is back.{/if}
  </p>
  {#if paused.length > 0}
    <ul class="paused" data-testid="host-offline-paused" aria-label="Paused sessions on {alias}">
      {#each pausedShown as name, i (i)}
        <li><span class="word">Paused</span> {name}</li>
      {/each}
      {#if pausedMore > 0}<li class="muted">and {pausedMore} more</li>{/if}
    </ul>
  {/if}
  {#if code}
    <details class="details">
      <summary>Details</summary>
      <code data-testid="host-offline-code">{code}</code>
    </details>
  {/if}
  {#if ontry || onopen || onshow || onwake}
    <div class="actions">
      {#if onwake}
        <button
          type="button"
          class="btn"
          disabled={waking}
          data-testid="host-offline-wake"
          onclick={onwake}>{waking ? 'Waking…' : 'Wake host'}</button>
      {/if}
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
      {#if onshow}
        <button type="button" class="btn" data-testid="host-offline-show" onclick={onshow}>Show sessions</button>
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
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--usage-warn) 7%, transparent);
    font-size: var(--text-2xs);
  }
  .head,
  .body {
    margin: 0;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  .muted, .body {
    color: var(--fg-muted);
  }
  .details summary {
    cursor: pointer;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .details code {
    font-size: var(--text-2xs);
  }
  .actions {
    display: flex;
    gap: 0.4rem;
  }
  .paused {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    font-family: var(--font-mono);
  }
  .paused .word {
    font-family: var(--font-sans);
    color: var(--status-idle);
  }
</style>
