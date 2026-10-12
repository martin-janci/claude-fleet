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
    /** `alert`: a red mark after the label, its text the mark's aria-label
     *  ("disk almost full"); its testid is the chip's plus `-alert`. */
    options: readonly { id: T; label: string; title?: string; dot?: 'on' | 'off'; alert?: string }[];
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
        {#if o.dot}<span class="dot dot-{o.dot}" aria-hidden="true"></span>{/if}{o.label}{#if o.alert}<span
            class="alert"
            aria-label={o.alert}
            data-testid={testidFor ? `${testidFor(o.id)}-alert` : undefined}
          ></span>{/if}
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
    background: var(--status-done);
  }
  .dot-off {
    background: var(--status-failed);
  }
  .alert {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    margin-left: 4px;
    background: var(--danger);
  }
</style>
