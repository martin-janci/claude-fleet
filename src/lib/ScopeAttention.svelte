<script lang="ts">
  // Needs-you is never hidden by scope (work graph M5, plan decision 6).
  // While a scope is chosen, sessions waiting on the person in OTHER scopes
  // get one line each — "2 need you in Personal →" — that switches to that
  // scope. This is the view only: nothing here relaxes the hub's boundary.
  import { sessions } from './sessions';
  import { attentionIdleMinutes } from './notify';
  import { effectiveScope, scopes, scopeOf, scopeFilter, needsYouElsewhere } from './orgs';

  let nowSec = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });
  const elsewhere = $derived(
    needsYouElsewhere($sessions, $effectiveScope, $scopes, $scopeOf, {
      idleSecs: $attentionIdleMinutes * 60,
      now: nowSec,
    }),
  );
</script>

{#if elsewhere.length > 0}
  <div class="elsewhere" data-testid="needs-you-elsewhere">
    {#each elsewhere as e (e.scope)}
      <button
        class="pill hot"
        data-testid="needs-you-elsewhere-item"
        onclick={() => scopeFilter.set(e.scope)}
        title="Switch to {e.label}"
      >
        {e.count} need{e.count === 1 ? 's' : ''} you in {e.label} →
      </button>
    {/each}
  </div>
{/if}

<style>
  .elsewhere {
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
    border-color: var(--warn, #f59e0b);
    color: var(--warn, #f59e0b);
  }
</style>
