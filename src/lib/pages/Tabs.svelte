<script lang="ts">
  // A tablist (WAI-ARIA tabs pattern): arrow keys move between tabs, Home /
  // End jump to the ends, and only the selected tab is in the tab order.
  let {
    tabs,
    selected = $bindable(0),
    label,
    testidPrefix = 'tab',
  }: {
    tabs: string[];
    selected?: number;
    label: string;
    testidPrefix?: string;
  } = $props();

  let buttons = $state<HTMLButtonElement[]>([]);

  function onKey(e: KeyboardEvent, i: number) {
    let next = i;
    if (e.key === 'ArrowRight') next = (i + 1) % tabs.length;
    else if (e.key === 'ArrowLeft') next = (i - 1 + tabs.length) % tabs.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = tabs.length - 1;
    else return;
    e.preventDefault();
    selected = next;
    buttons[next]?.focus();
  }
</script>

<div class="tabs" role="tablist" aria-label={label}>
  {#each tabs as title, i (i)}
    <button
      bind:this={buttons[i]}
      type="button"
      role="tab"
      class="tab"
      aria-selected={selected === i}
      tabindex={selected === i ? 0 : -1}
      data-testid={`${testidPrefix}-${i}`}
      onclick={() => (selected = i)}
      onkeydown={(e) => onKey(e, i)}>{title}</button
    >
  {/each}
</div>

<style>
  .tabs {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 0.75rem;
  }
  .tab {
    background: transparent;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--control-font);
    padding: 0.35rem 0.7rem;
    cursor: pointer;
  }
  .tab[aria-selected='true'] {
    color: var(--fg);
    border-bottom-color: var(--accent);
  }
  .tab:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: calc(-1 * var(--ring-w));
  }
</style>
