<script lang="ts">
  // Work graph M12.4: a tracker a person has to act on — its token expired
  // or was revoked, or the site wants a browser login — gets one line in the
  // sidebar's attention strip, "Reconnect Jira (acme)", which opens Settings
  // → Work. Read from the live `trackers` store (a `work:tracker` frame
  // updates it the moment a sync fails), so it needs no polling, and it
  // goes away by itself once the tracker syncs again.
  import { trackers, trackerAttention } from './trackers';
  import { openSettingsAt } from './app_views';

  const items = $derived(trackerAttention($trackers));
</script>

{#if items.length > 0}
  <div class="tracker-attention" data-testid="tracker-attention">
    {#each items as item (item.trackerId)}
      <button
        class="pill hot"
        data-testid="tracker-attention-item"
        title={item.detail}
        onclick={() => openSettingsAt('work')}
      >
        ⚠ {item.label} →
      </button>
    {/each}
  </div>
{/if}

<style>
  .tracker-attention {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
  }
  .pill {
    font-size: 0.72rem;
    padding: 0.1rem 0.45rem;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    cursor: pointer;
  }
  .pill.hot {
    border-color: var(--danger, #e64a4a);
    color: var(--danger, #e64a4a);
  }
</style>
