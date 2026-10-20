<script lang="ts">
  // "Check for updates" (Settings → About): a person's ask, so the hub
  // re-reads the published channel before it answers (`checkForUpdate(true)`).
  // An update it finds shows in the window's banner, as one found on the
  // schedule does; this only says what the ask came back with. A person's
  // check brings back a banner they dismissed earlier, so the update it
  // finds has its Restart / Download button again (review w26-9).
  import Loader from './Loader.svelte';
  import { checkForUpdate, checkedLine, dismissUpdate } from './updates';

  let busy = $state(false);
  let line = $state<string | null>(null);

  async function check() {
    busy = true;
    line = null;
    const r = await checkForUpdate(true);
    busy = false;
    if (r.ok) dismissUpdate(null);
    line = r.ok ? checkedLine(r.value) : `Could not check: ${r.error.message}`;
  }
</script>

<div class="hook-actions" data-testid="update-check">
  <button class="hook-btn" onclick={check} disabled={busy} data-testid="update-check-btn">
    {#if busy}<Loader name="comet" size={12} class="btn-loader" />{/if}{busy ? 'Checking…' : 'Check for updates'}
  </button>
  {#if line}<span class="hook-desc" data-testid="update-check-line">{line}</span>{/if}
</div>
