<!-- The 8 px status dot. Colour never carries the state alone: it has a
     label, or `label={null}` when a word beside it already says the state
     (manual: StatusChip). -->
<script lang="ts">
  import { STATE_WORD, type OfState } from './status';

  let { state = 'idle', label, size }: { state?: OfState; label?: string | null; size?: number } = $props();
</script>

<span
  class="of-dot"
  class:waiting={state === 'waiting'}
  class:working={state === 'working'}
  class:failed={state === 'failed'}
  class:done={state === 'done'}
  role={label === null ? undefined : 'img'}
  aria-label={label === null ? undefined : (label ?? STATE_WORD[state])}
  aria-hidden={label === null ? 'true' : undefined}
  style:width={size ? `${size}px` : undefined}
  style:height={size ? `${size}px` : undefined}
></span>
