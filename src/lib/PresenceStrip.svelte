<script lang="ts">
  // Who else is looking at the open session (redesign 11.7b), drawn in the
  // New layout's session header: one initial per person, the name and device
  // on hover, and "+N" past three. Nothing when nobody else is there.
  import { othersLooking, presence, viewerTitle } from './presence';

  let { sessionId }: { sessionId: number } = $props();

  const others = $derived($presence && $presence.session_id === sessionId ? othersLooking($presence) : []);
  const shown = $derived(others.slice(0, 3));
  const more = $derived(others.length - shown.length);
  const label = $derived(
    others.length === 1
      ? `${others[0].name} is also looking`
      : `${others.length} others are also looking: ${others.map((v) => v.name).join(', ')}`,
  );

  function initial(name: string): string {
    return (name.trim()[0] ?? '?').toUpperCase();
  }
</script>

{#if others.length > 0}
  <span class="presence" role="status" aria-label={label} data-testid="presence-strip">
    {#each shown as v (v.person_id)}
      <span class="face" title={viewerTitle(v)} data-testid="presence-face">{initial(v.name)}</span>
    {/each}
    {#if more > 0}<span class="face more" title={label}>+{more}</span>{/if}
    <span class="word">{others.length === 1 ? `${others[0].name} watching` : `${others.length} watching`}</span>
  </span>
{/if}

<style>
  .presence {
    display: inline-flex;
    align-items: center;
    gap: 0.15rem;
    font-size: 0.8em;
    color: var(--fg-muted);
    white-space: nowrap;
  }
  .face {
    display: inline-grid;
    place-items: center;
    width: 1.35rem;
    height: 1.35rem;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--fg);
    font-weight: 600;
    font-size: 11px;
    box-shadow: 0 0 0 2px var(--bg-pane);
  }
  .face + .face {
    margin-left: -0.35rem;
  }
  .more {
    background: var(--chip-bg);
  }
  .word {
    margin-left: 0.3rem;
  }
</style>
