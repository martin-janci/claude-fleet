<script lang="ts">
  // An `E_CONFLICT` as the Work view shows it (work graph M14): what changed,
  // the current value the hub answered with, and Reload — never a silent
  // overwrite. Inline, so it sits inside a notice line or an item's error.
  import type { ConflictNotice } from './work_view';

  let { notice, onreload }: { notice: ConflictNotice; onreload?: () => void } = $props();
</script>

<span class="conflict" data-testid="work-conflict">
  <span>{notice.text}</span>
  {#if notice.current}<span class="current" data-testid="work-conflict-current">{notice.current}</span>{/if}
  {#if onreload}
    <button class="btn btn--quiet" type="button" data-testid="work-conflict-reload" onclick={() => onreload?.()}>Reload</button>
  {/if}
</span>

<style>
  .conflict {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    align-items: baseline;
  }
  .current {
    font-weight: 600;
  }
</style>
