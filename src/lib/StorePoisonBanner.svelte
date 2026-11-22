<script lang="ts">
  // Review w19-1: a panic that held the store's lock poisons it, and from
  // then on every command answers E_LOCK until the app restarts. The hub
  // exits so its supervisor restarts it; the desktop's backend says so
  // instead (`app:store-poisoned`, every few seconds from
  // `app_events::watch_store_poison`), and this blocks the window with the
  // one thing that helps.
  import { onDestroy, onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { invokeCmd } from './result';

  const STORE_POISONED_EVENT = 'app:store-poisoned';

  let poisoned = $state(false);
  let restarting = $state(false);
  let error = $state<string | null>(null);
  let unlisten: (() => void) | null = null;
  let gone = false;

  onMount(() => {
    void listen(STORE_POISONED_EVENT, () => {
      poisoned = true;
    }).then((off) => {
      if (gone) off();
      else unlisten = off;
    });
  });
  onDestroy(() => {
    gone = true;
    unlisten?.();
  });

  async function restart() {
    restarting = true;
    error = null;
    const r = await invokeCmd<void>('restart_app');
    // A restart does not come back; an answer at all means it did not run.
    restarting = false;
    if (!r.ok) error = 'Restarting failed. Quit Orbit Fleet and open it again.';
  }
</script>

{#if poisoned}
  <div class="poison-backdrop" data-testid="store-poison-banner">
    <div class="poison-card" role="alertdialog" aria-modal="true" aria-labelledby="store-poison-title">
      <h2 id="store-poison-title">Orbit Fleet hit an internal error and must restart</h2>
      <p>Until it does, every action fails. Your sessions keep running on their hosts.</p>
      <button type="button" class="act" data-testid="store-poison-restart" disabled={restarting} onclick={restart}>
        {restarting ? 'Restarting…' : 'Restart'}
      </button>
      {#if error}<p class="error" role="status">{error}</p>{/if}
    </div>
  </div>
{/if}

<style>
  .poison-backdrop {
    position: fixed;
    inset: 0;
    z-index: 10000;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 16px;
    background: color-mix(in srgb, var(--bg) 70%, transparent);
  }
  .poison-card {
    max-width: 28rem;
    padding: 1.25rem 1.5rem;
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
  }
  h2 {
    margin: 0 0 0.5rem;
    font-size: var(--text-md);
  }
  p {
    margin: 0 0 1rem;
    font-size: var(--text-sm);
  }
  .act {
    font-weight: 600;
  }
  .error {
    margin: 0.75rem 0 0;
    color: var(--danger);
  }
</style>
