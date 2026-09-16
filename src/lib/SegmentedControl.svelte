<script lang="ts" generics="T extends string">
  // A row of mutually exclusive toggle buttons (aria-pressed) that also
  // answers Left/Right like a native segmented control. `via` tells the
  // owner whether the user chose a segment (click, Enter, Space) or only
  // arrowed onto it, so it can decide whether to move focus elsewhere.
  let {
    options,
    value,
    label,
    testidPrefix,
    disabled = false,
    onchange,
  }: {
    options: readonly { id: T; label: string }[];
    value: T;
    /** Accessible name of the group. */
    label: string;
    /** Each button gets `data-testid="<prefix><id>"`. */
    testidPrefix: string;
    disabled?: boolean;
    onchange: (id: T, via: 'click' | 'arrow') => void;
  } = $props();

  let root: HTMLElement | undefined = $state();

  function onkeydown(e: KeyboardEvent) {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    e.preventDefault();
    const i = options.findIndex((o) => o.id === value);
    const next = options[(i + (e.key === 'ArrowRight' ? 1 : options.length - 1)) % options.length];
    onchange(next.id, 'arrow');
    root?.querySelector<HTMLElement>(`[data-testid="${testidPrefix}${next.id}"]`)?.focus();
  }
</script>

<div class="seg" role="group" aria-label={label} bind:this={root}>
  {#each options as o (o.id)}
    <button
      type="button"
      class="seg-pick"
      class:active={value === o.id}
      aria-pressed={value === o.id}
      {disabled}
      data-testid="{testidPrefix}{o.id}"
      onclick={() => onchange(o.id, 'click')}
      {onkeydown}
    >{o.label}</button>
  {/each}
</div>

<style>
  .seg {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 4px;
    overflow: hidden;
  }
  .seg-pick {
    flex: 1 1 0;
    font-size: 0.75rem;
    padding: 0.3rem 0.4rem;
    border: 0;
    border-right: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
  }
  .seg-pick:last-child { border-right: 0; }
  .seg-pick.active { color: var(--fg); background: color-mix(in srgb, var(--accent) 14%, transparent); }
  /* `.seg` clips its children, so draw the focus ring inside the button. */
  .seg-pick:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  .seg-pick:disabled { cursor: not-allowed; }
</style>
