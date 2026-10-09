<script lang="ts">
  /**
   * A pop-out terminal window's whole page (redesign step 5.4): one
   * session's agent pane or shell terminal and nothing else. It loads what
   * the pane reads (the hub status, sessions, hosts and this client's
   * grants), follows their row events, picks the session without touching
   * the main window's remembered selection, and shows the same
   * `TerminalView` the main window does, attached under the window's own
   * label.
   */
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import TerminalView from './TerminalView.svelte';
  import Toasts from './Toasts.svelte';
  import { loadHubStatus, hubStatus } from './hub';
  import { loadSessions, applySessionEvents, sessions } from './sessions';
  import { loadHosts, applyHostEvents } from './hosts';
  import { loadMyGrants, applyGrantChanges } from './access';
  import { subscribeToRowEvents } from './events';
  import { selectSession, selectedSession } from './selection';
  import type { PopoutTarget } from './terminal_popout';

  let { popout }: { popout: PopoutTarget } = $props();

  /** `loading`, then `ready`; `gone` when the session is not in the fleet
   *  (killed while the window was opening), `unavailable` when no hub
   *  answers. */
  let phase: 'loading' | 'ready' | 'gone' | 'unavailable' = $state('loading');
  let unlisten: UnlistenFn | null = null;
  let destroyed = false;

  onMount(async () => {
    await loadHubStatus();
    if (destroyed) return;
    if (get(hubStatus).unavailable) {
      phase = 'unavailable';
      return;
    }
    // Subscribed before the first list, as App does, so no update is lost.
    const off = await subscribeToRowEvents({
      onSessionEvents: applySessionEvents,
      onHostEvents: applyHostEvents,
      onGrantChanged: applyGrantChanges,
    });
    if (destroyed) {
      off();
      return;
    }
    unlisten = off;
    const [sr] = await Promise.all([loadSessions(), loadHosts(), loadMyGrants()]);
    if (destroyed) return;
    const row = sr.ok ? get(sessions).find((s) => s.id === popout.sessionId) : undefined;
    if (!row) {
      phase = 'gone';
      return;
    }
    selectSession(row, { follow: true, remember: false });
    phase = 'ready';
  });

  // The session leaving the fleet clears the selection; say so rather than
  // showing the main window's empty-state words.
  $effect(() => {
    if (phase === 'ready' && !$selectedSession) phase = 'gone';
  });

  onDestroy(() => {
    destroyed = true;
    unlisten?.();
  });
</script>

<main class="popout" data-testid="terminal-popout-window">
  {#if phase === 'ready'}
    <TerminalView popout={popout.label} shell={popout.shell ?? undefined} />
  {:else if phase === 'loading'}
    <p class="note" data-testid="terminal-popout-loading">Opening the terminal…</p>
  {:else if phase === 'gone'}
    <p class="note" data-testid="terminal-popout-gone">This session is no longer in the fleet. Close this window.</p>
  {:else}
    <p class="note" data-testid="terminal-popout-unavailable">The hub is not answering. The main window says why.</p>
  {/if}
</main>
<Toasts />

<style>
  .popout {
    display: flex;
    flex-direction: column;
    height: 100vh;
    min-height: 0;
    background: var(--bg);
    color: var(--fg);
  }
  .popout :global(.term-root) {
    flex: 1 1 auto;
    min-height: 0;
  }
  .note {
    margin: auto;
    padding: 1rem;
    color: var(--fg-muted);
    font-size: 0.9rem;
    text-align: center;
  }
</style>
