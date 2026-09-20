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

<div class="btn-group seg" role="group" aria-label={label} bind:this={root}>
  {#each options as o (o.id)}
    <button
      type="button"
      class="btn btn--chip btn--toggle seg-pick"
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
    border-radius: 4px;
    overflow: hidden;
  }
  /* `.btn--chip` gives every segment its own resting border; collapse the
     shared edge instead of doubling it. */
  .seg-pick {
    flex: 1 1 0;
    border-radius: 0;
  }
  .seg-pick + .seg-pick {
    margin-left: -1px;
  }
</style>
