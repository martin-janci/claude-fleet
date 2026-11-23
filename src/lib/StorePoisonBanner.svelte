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
  import Modal from './Modal.svelte';

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
  <!-- A <Modal> (review w14-6): showModal() puts it in the top layer above
       any dialog already open, makes the dead UI behind it inert and traps
       focus, which a z-index div did not. No `onclose`: Escape and the
       backdrop do nothing, only Restart helps. -->
  <Modal label="Orbit Fleet must restart" closeOnBackdrop={false} testid="store-poison-banner">
    <div class="poison-card" role="alert" aria-labelledby="store-poison-title">
      <h2 id="store-poison-title">Orbit Fleet hit an internal error and must restart</h2>
      <p>Until it does, every action fails. Your sessions keep running on their hosts.</p>
      <button type="button" class="act" data-testid="store-poison-restart" data-autofocus disabled={restarting} onclick={restart}>
        {restarting ? 'Restarting…' : 'Restart'}
      </button>
      {#if error}<p class="error" role="status">{error}</p>{/if}
    </div>
  </Modal>
{/if}

<style>
  .poison-card {
    max-width: 28rem;
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
