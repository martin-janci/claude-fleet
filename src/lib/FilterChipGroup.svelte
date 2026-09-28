<script lang="ts" generics="T extends string | number">
  // One labelled, single-choice filter: a wrapping row of pressed-state
  // chips (hosts, trackers, statuses — lists of any length). The label sits
  // above, so every group in the filter panel reads the same way.
  let {
    label,
    options,
    value,
    onchange,
    testidFor,
    hint,
  }: {
    label: string;
    options: readonly { id: T; label: string; title?: string; dot?: 'on' | 'off' }[];
    value: T;
    onchange: (id: T) => void;
    testidFor?: (id: T) => string;
    /** A short note after the label ("group by work only"). */
    hint?: string;
  } = $props();
</script>

<div class="fgroup" role="group" aria-label={label}>
  <span class="fgroup-label">{label}{#if hint}<span class="hint"> · {hint}</span>{/if}</span>
  <div class="chips">
    {#each options as o (o.id)}
      <button
        type="button"
        class="btn btn--chip btn--toggle"
        aria-pressed={value === o.id}
        title={o.title}
        data-testid={testidFor?.(o.id)}
        onclick={() => onchange(o.id)}
      >
        {#if o.dot}<span class="dot dot-{o.dot}" aria-hidden="true"></span>{/if}{o.label}
      </button>
    {/each}
  </div>
</div>

<style>
  .fgroup {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .fgroup-label {
    font-size: var(--control-font-sm);
    font-weight: 600;
    color: var(--fg-muted);
  }
  .hint {
    font-weight: 400;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    margin-right: 2px;
  }
  .dot-on {
    background: rgb(80, 200, 110);
  }
  .dot-off {
    background: rgb(220, 130, 130);
  }
</style>
