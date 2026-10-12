<script lang="ts" generics="T extends string">
  // A row of mutually exclusive toggle buttons (aria-pressed) that also
  // answers Left/Right like a native segmented control. `via` tells the
  // owner whether the user chose a segment (click, Enter, Space) or only
  // arrowed onto it, so it can decide whether to move focus elsewhere.
  // Its look is `.seg-group` in controls.css.
  let {
    options,
    value,
    label,
    testidPrefix,
    disabled = false,
    onchange,
  }: {
    /** `testid` overrides the `<prefix><id>` default for one segment. */
    options: readonly {
      id: T;
      label: string;
      title?: string;
      testid?: string;
      /** This segment alone is off (the group's `disabled` turns them all off). */
      disabled?: boolean;
      /** `aria-keyshortcuts` for the chord that flips to it. */
      keyshortcuts?: string;
    }[];
    value: T;
    /** Accessible name of the group. */
    label: string;
    /** Each button gets `data-testid="<prefix><id>"` unless its option names one. */
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
    if (next.disabled) return;
    onchange(next.id, 'arrow');
    root?.querySelectorAll<HTMLElement>('button')[options.indexOf(next)]?.focus();
  }
</script>

<div class="btn-group seg-group" role="group" aria-label={label} bind:this={root}>
  {#each options as o (o.id)}
    <button
      type="button"
      class="btn btn--chip btn--toggle"
      aria-pressed={value === o.id}
      disabled={disabled || o.disabled}
      aria-keyshortcuts={o.keyshortcuts}
      title={o.title}
      data-testid={o.testid ?? `${testidPrefix}${o.id}`}
      onclick={() => onchange(o.id, 'click')}
      {onkeydown}
    >{o.label}</button>
  {/each}
</div>
